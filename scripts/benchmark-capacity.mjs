// Real browser H264 decoding on independent peers, grouped to avoid tab overhead.
// Only the isolated Rust fixture is permitted. This measures local fan-out, not NIC capacity.
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";

const base = "http://127.0.0.1:8091", service = "uvms-rust-opt-bench";
const counts = (process.env.UVMS_CAP_COUNTS || "32,64,128,256").split(",").map(Number);
const seconds = Number(process.env.UVMS_CAP_SECONDS || 60);
const groupSize = Number(process.env.UVMS_CAP_GROUP || 8);
const output = process.env.UVMS_CAP_OUTPUT || "test-results/capacity";
const softwareDecode = process.env.UVMS_CAP_SOFTWARE_DECODE === "1";
const peersPerBrowser=Number(process.env.UVMS_CAP_BROWSER_PEERS||1024);
if(!Number.isInteger(peersPerBrowser)||peersPerBrowser<groupSize||peersPerBrowser%groupSize)throw Error("Browser peer limit must be a multiple of group size");
if (counts.some((n,i)=>!Number.isInteger(n)||n<1||n>1024||(i&&n<=counts[i-1])) || seconds<10||seconds>300 || !Number.isInteger(groupSize)||groupSize<1||groupSize>16) throw Error("Invalid bounded parameters");
const health = await (await fetch(base+"/healthz")).json();
if (health.media_engine!=="rust-native") throw Error("Expected Rust fixture");
await mkdir(output,{recursive:true});
const result={health,base,service,counts,seconds,groupSize,scope:"Synthetic H264 640x360 nominal 15fps, independent WebRTC peers with video decoding, one source, HLS cache warm, Windows browser and WSL server share hardware",thresholds:{minFps:13,maxLossRatio:0.001,maxMeanJitterBufferSeconds:0.25},stages:[],startedAt:new Date().toISOString()};
const samples=[],errors=[];
const sampler=spawn("wsl",["-d","Ubuntu-24.04","-u","root","--exec","env","UVMS_BENCH_SERVICE="+service,"UVMS_BENCH_SOURCE_PORT=8554","python3","-u","/mnt/d/workspaces/uvms/scripts/sample-service.py"],{stdio:["ignore","pipe","pipe"]});
let pending="";
sampler.stdout.on("data",data=>{pending+=data;let end;while((end=pending.indexOf("\n"))>=0){const line=pending.slice(0,end);pending=pending.slice(end+1);try{samples.push(JSON.parse(line));}catch{}}});
sampler.stderr.on("data",data=>errors.push(String(data)));
result.softwareDecode=softwareDecode;
result.peersPerBrowser=peersPerBrowser;
const browsers=[],cdps=[],pages=[];
const cpuSnapshot=async()=>({processInfo:(await Promise.all(cdps.map(c=>c.send("SystemInfo.getProcessInfo")))).flatMap(r=>r.processInfo)});
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
const mean=values=>values.reduce((a,b)=>a+b,0)/values.length;
async function save(){await writeFile(output+"/capacity.json",JSON.stringify({...result,errors},null,2));await writeFile(output+"/service-samples.json",JSON.stringify(samples,null,2));}
async function newPage(){
  if(pages.length%(peersPerBrowser/groupSize)===0){
    const browser=await chromium.launch({executablePath:process.env.UVMS_BROWSER_PATH||"C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",headless:true,args:["--autoplay-policy=no-user-gesture-required","--disable-background-timer-throttling","--disable-renderer-backgrounding","--disable-backgrounding-occluded-windows",...(softwareDecode?["--disable-accelerated-video-decode"]:[])]});
    browsers.push(browser);cdps.push(await browser.newBrowserCDPSession());
  }
  const page=await browsers.at(-1).newPage({viewport:{width:1280,height:720}});
  // Use a real navigation so Chromium recognizes the local network origin.
  // Suppress only the UI script, which would otherwise add its own connection.
  await page.route(base+"/app.js",route=>route.fulfill({contentType:"text/javascript",body:""}));
  page.on("pageerror",error=>errors.push(error.message));
  page.on("console",message=>{if(message.type()==="error")errors.push(message.text());});
  const network=await page.context().newCDPSession(page);
  await network.send("Network.enable");
  network.on("Network.webSocketHandshakeResponseReceived",({response})=>{if(response.status!==101)errors.push(`WebSocket HTTP ${response.status}`);});
  await page.goto(base);
  await page.setContent("<!doctype html><title>UVMS capacity test</title><body></body>");
  await page.evaluate(()=>{
    window.peers=[];
    window.addPeer=()=>new Promise((resolve,reject)=>{
      const pc=new RTCPeerConnection(), video=document.createElement("video");
      video.muted=true;video.autoplay=true;video.playsInline=true;video.width=320;video.height=180;
      document.body.append(video);
      const entry={pc,video,errors:[],started:performance.now(),ready:false};peers.push(entry);
      const timer=setTimeout(()=>reject(Error("Peer startup exceeded 45 seconds")),45000);
      const fail=error=>{entry.errors.push(error.message||String(error));clearTimeout(timer);reject(error);};
      pc.addTransceiver("video",{direction:"recvonly"});pc.addTransceiver("audio",{direction:"recvonly"});
      const tracks=new MediaStream();
      pc.ontrack=event=>{tracks.addTrack(event.track);video.srcObject=tracks;video.play().catch(fail);};
      pc.onconnectionstatechange=()=>{if(pc.connectionState==="failed")fail(Error("ICE connection failed"));};
      const ws=new WebSocket(location.origin.replace("http:","ws:")+"/webrtc_playnow/singleport/udp/camera");entry.ws=ws;
      let answered=false;
      ws.onmessage=async event=>{try{
        if(event.data.startsWith("H264|-|-|")){
          pc.getTransceivers()[0].setCodecPreferences(RTCRtpReceiver.getCapabilities("video").codecs.filter(c=>c.mimeType.toLowerCase()==="video/h264"));
          const offer=await pc.createOffer();offer.sdp=offer.sdp.replaceAll("a=extmap-allow-mixed\r\n","");await pc.setLocalDescription(offer);
          if(pc.iceGatheringState!=="complete")await new Promise((done,no)=>{const t=setTimeout(()=>no(Error("ICE gather timeout")),8000);pc.addEventListener("icegatheringstatechange",()=>{if(pc.iceGatheringState==="complete"){clearTimeout(t);done();}});});
          ws.send(JSON.stringify({type:"offer",sdp:pc.localDescription.sdp}));
        }else{
          const parts=event.data.split("|-|-|"),answer=JSON.parse(parts[0]);
          if(answer.type!=="answer")throw Error(answer.message||"Invalid answer");
          answered=true;await pc.setRemoteDescription(answer);if(parts[1])await pc.addIceCandidate(JSON.parse(parts[1]));ws.close();
          const check=async()=>{try{const stats=[...(await pc.getStats()).values()];const v=stats.find(s=>s.type==="inbound-rtp"&&s.kind==="video");
            if(v?.framesDecoded>=10){entry.ready=true;entry.startupMs=performance.now()-entry.started;clearTimeout(timer);resolve();}
            else if(performance.now()-entry.started<45000)setTimeout(check,100);
          }catch(error){fail(error);}};check();
        }
      }catch(error){fail(error);}};
      ws.onerror=()=>{if(!answered)fail(Error("Signaling WebSocket error"));};
      ws.onclose=()=>{if(!answered)fail(Error("Signaling ended before answer"));};
    });
    window.snapshot=()=>Promise.all(peers.map(async({pc,video,errors,startupMs})=>{
      const stats=await pc.getStats(),values=[...stats.values()],v=values.find(s=>s.type==="inbound-rtp"&&s.kind==="video");
      const transport=stats.get(v?.transportId),pair=stats.get(transport?.selectedCandidatePairId),candidate=stats.get(pair?.remoteCandidateId);
      return {time:performance.now()/1000,state:pc.connectionState,width:video.videoWidth,height:video.videoHeight,errors,startupMs,port:candidate?.port,
        frames:v?.framesDecoded||0,receivedFrames:v?.framesReceived||0,decoder:v?.decoderImplementation,powerEfficientDecoder:v?.powerEfficientDecoder,bytes:v?.bytesReceived||0,packets:v?.packetsReceived||0,lost:v?.packetsLost||0,dropped:v?.framesDropped||0,
        freezes:v?.freezeCount||0,freezeSeconds:v?.totalFreezesDuration||0,jitterDelay:v?.jitterBufferDelay||0,jitterEmitted:v?.jitterBufferEmittedCount||0,decodeSeconds:v?.totalDecodeTime||0};
    }));
  });
  pages.push(page);return page;
}
const snapshot=async()=>(await Promise.all(pages.map(page=>page.evaluate(()=>snapshot())))).flat();
let current=0;
try{
  await fetch(base+"/hls/camera/index.m3u8");
  for(const count of counts){
    const stage={viewers:count,passed:false,startedAt:new Date().toISOString()};result.stages.push(stage);
    try{
      while(current<count){
        const page=current%groupSize===0?await newPage():pages.at(-1);
        const n=Math.min(4,count-current,groupSize-current%groupSize);
        await page.evaluate(n=>Promise.all(Array.from({length:n},()=>addPeer())),n);current+=n;
        if(current%32===0)console.log(JSON.stringify({connected:current,target:count}));
      }
      await delay(5000);
      await fetch(base+"/hls/camera/index.m3u8");
      const before=await snapshot(),cpuBefore=await cpuSnapshot(),start=Date.now()/1000;
      stage.intervals=[];let previous=before;
      for(let elapsed=0;elapsed<seconds;elapsed+=10){
        await delay(Math.min(10,seconds-elapsed)*1000);await fetch(base+"/hls/camera/index.m3u8");
        const now=await snapshot();stage.intervals.push({at:Date.now()/1000,minFps:Math.min(...now.map((a,i)=>(a.frames-previous[i].frames)/(a.time-previous[i].time))),connected:now.filter(a=>a.state==="connected").length,stalled:now.filter((a,i)=>a.frames===previous[i].frames).length});previous=now;
      }
      const after=previous,end=Date.now()/1000,cpuAfter=await cpuSnapshot();
      const metrics=samples.filter(s=>s.time>=start&&s.time<=end&&s.cpu_percent_one_core!==undefined);if(!metrics.length)throw Error("No service resource samples");
      stage.clients=after.map((a,i)=>{const b=before[i],dt=a.time-b.time,frames=a.frames-b.frames,packets=a.packets-b.packets,lost=a.lost-b.lost;return {...a,fps:frames/dt,mbps:8*(a.bytes-b.bytes)/dt/1e6,frames,packets,lost,lossRatio:Math.max(0,lost)/Math.max(1,packets+lost),dropped:a.dropped-b.dropped,freezes:a.freezes-b.freezes,freezeSeconds:a.freezeSeconds-b.freezeSeconds,meanJitterBufferSeconds:(a.jitterDelay-b.jitterDelay)/Math.max(1,a.jitterEmitted-b.jitterEmitted),decodeMsPerFrame:1000*(a.decodeSeconds-b.decodeSeconds)/Math.max(1,frames)};});
      stage.cpuMeanOneCorePercent=mean(metrics.map(s=>s.cpu_percent_one_core));stage.cpuPeakOneCorePercent=Math.max(...metrics.map(s=>s.cpu_percent_one_core));stage.rssPeakMiB=Math.max(...metrics.map(s=>s.rss_mib));
      stage.sourceInputsMax=Math.max(...metrics.map(s=>s.source_inputs));stage.udpPorts=[...new Set(metrics.flatMap(s=>s.udp_ports))];
      stage.browserCpuOneCorePercent=100*cpuAfter.processInfo.reduce((total,p)=>{const old=cpuBefore.processInfo.find(q=>q.id===p.id);return total+(old?Math.max(0,p.cpuTime-old.cpuTime):0);},0)/(end-start);
      stage.browserProcessCount=cpuAfter.processInfo.length;
      stage.meanFps=mean(stage.clients.map(c=>c.fps));stage.minFps=Math.min(...stage.clients.map(c=>c.fps));stage.totalMbps=stage.clients.reduce((s,c)=>s+c.mbps,0);
      stage.totalLost=stage.clients.reduce((s,c)=>s+Math.max(0,c.lost),0);stage.totalFreezes=stage.clients.reduce((s,c)=>s+c.freezes,0);
      stage.failedClients=stage.clients.filter(c=>c.state!=="connected"||c.width!==640||c.height!==360||c.fps<result.thresholds.minFps||c.lossRatio>result.thresholds.maxLossRatio||c.meanJitterBufferSeconds>result.thresholds.maxMeanJitterBufferSeconds||c.errors.length).length;
      stage.passed=stage.failedClients===0&&stage.intervals.every(i=>i.stalled===0)&&errors.length===0;
    }catch(error){stage.error=error.message;}
    stage.finishedAt=new Date().toISOString();await save();console.log(JSON.stringify({...stage,clients:undefined}));
    if(!stage.passed)break;
  }
}catch(error){result.error=error.message;}
finally{
  await Promise.all(browsers.map(browser=>browser.close()));sampler.kill();result.finishedAt=new Date().toISOString();
  result.passed=result.stages.length===counts.length&&result.stages.every(s=>s.passed);await save();
}
if(!result.passed)process.exitCode=1;
