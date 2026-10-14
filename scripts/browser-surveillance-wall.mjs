// Read-only four-camera wall verification. Never changes camera/recording settings
// or starts/stops services. Synthetic fixture aliases only.
import { chromium } from "playwright";
import { execFileSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

const base=process.env.UVMS_TEST_URL || "http://127.0.0.1:8092/";
const service=process.env.UVMS_TEST_SERVICE || "uvms-surveillance";
const sourcePort=Number(process.env.UVMS_TEST_SOURCE_PORT || 18554);
const udpPort=Number(process.env.UVMS_TEST_UDP_PORT || 51200);
const aliases=["aac","pcmu","pcma","opus"];
const output=path.resolve(process.env.UVMS_WALL_EVIDENCE || "test-results/rust-surveillance/wall");
await mkdir(output,{recursive:true});
const linuxSnapshot=()=>JSON.parse(execFileSync("wsl",["-d","Ubuntu-24.04","-u","root","--exec","python3","-c",`
import json,pathlib,subprocess,sys
pid=int(subprocess.check_output(['systemctl','show','-p','MainPID','--value',sys.argv[1]],text=True))
root=pathlib.Path('/proc')/str(pid)
inodes=set()
for fd in (root/'fd').iterdir():
 try:
  target=str(fd.readlink())
  if target.startswith('socket:['):inodes.add(target[8:-1])
 except OSError:pass
inputs=0
for row in (root/'net/tcp').read_text().splitlines()[1:]:
 p=row.split()
 if p[9] in inodes and p[3]=='01' and int(p[2].split(':')[1],16)==int(sys.argv[2]):inputs+=1
ports=[]
for table in ['udp','udp6']:
 for row in (root/'net'/table).read_text().splitlines()[1:]:
  p=row.split()
  if p[9] in inodes:ports.append(int(p[1].split(':')[1],16))
print(json.dumps({'pid':pid,'sourceInputs':inputs,'udpPorts':sorted(ports),'mediaUdpPorts':sorted(p for p in ports if p!=5353),'mdnsSockets':ports.count(5353)}))
`,service,String(sourcePort)],{encoding:"utf8",timeout:10000}));
const browser=await chromium.launch({executablePath:process.env.UVMS_BROWSER || "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",headless:true,args:["--autoplay-policy=no-user-gesture-required","--disable-features=WebRtcHideLocalIpsWithMdns"]});
const results=[];
const snapshot=page=>page.evaluate(()=>[...document.querySelectorAll(".camera-tile")].map(tile=>{
  const video=tile.querySelector("video"),quality=video.getVideoPlaybackQuality();
  return {id:tile.dataset.camera,state:tile.dataset.state,frames:quality.totalVideoFrames-quality.droppedVideoFrames,time:video.currentTime,width:video.videoWidth,height:video.videoHeight,muted:video.muted,audioTracks:video.srcObject?.getAudioTracks().length || 0,audioDecodedBytes:video.webkitAudioDecodedByteCount || 0,audioLabel:tile.querySelector(".camera-audio").textContent};
}));
try {
  for(const mode of ["webrtc","mse","hls"]){
    const context=await browser.newContext({viewport:{width:1440,height:1100}});
    await context.addInitScript(()=>{
      window.wallTestPeers=[];
      const Native=window.RTCPeerConnection;
      window.RTCPeerConnection=new Proxy(Native,{construct(target,args){const peer=Reflect.construct(target,args);window.wallTestPeers.push(peer);return peer;}});
    });
    const page=await context.newPage();
    const result={mode,passed:false,errors:[]};results.push(result);
    page.on("pageerror",error=>result.errors.push(error.message));
    try {
      await page.goto(base);
      await page.waitForSelector('#camera-choices input[value="opus"]',{state:"attached",timeout:15000});
      await page.locator('button[data-layout="4"]').click();
      for(const alias of aliases)await page.locator(`#camera-choices input[value="${alias}"]`).check();
      await page.locator("#mode").selectOption(mode);
      await page.locator("#connect").click();
      await page.waitForFunction(()=>document.querySelectorAll('.camera-tile[data-state="live"]').length===4 && [...document.querySelectorAll('.camera-tile video')].every(v=>v.videoWidth>0&&v.getVideoPlaybackQuality().totalVideoFrames>=15),null,{timeout:65000});
      result.before=await snapshot(page);result.linuxBefore=linuxSnapshot();
      await page.waitForTimeout(6000);
      result.after=await snapshot(page);result.linuxAfter=linuxSnapshot();
      for(const alias of aliases){
        const before=result.before.find(v=>v.id===alias),after=result.after.find(v=>v.id===alias);
        if(!before||!after||after.state!=="live"||after.frames-before.frames<25||after.time-before.time<2)throw Error(`${alias}: four-camera playback did not progress`);
        if(mode==="webrtc" && after.audioTracks!==1)throw Error(`${alias}: missing WebRTC audio track`);
        if(mode!=="webrtc" && after.audioDecodedBytes<=0)throw Error(`${alias}: AAC audio was not decoded`);
      }
      for(const observation of [result.linuxBefore,result.linuxAfter]){
        if(observation.sourceInputs!==4)throw Error(`Expected one source input per camera, saw ${observation.sourceInputs}`);
        if(observation.mediaUdpPorts.length!==1 || observation.mediaUdpPorts[0]!==udpPort)throw Error(`Expected one shared media UDP port ${udpPort}`);
      }
      result.peers=await page.evaluate(async()=>{
        const peers=[];
        for(const peer of window.wallTestPeers.filter(p=>p.connectionState==="connected")){
          const stats=await peer.getStats();const reports=[...stats.values()];
          const transport=reports.find(r=>r.type==="transport"&&r.selectedCandidatePairId);
          const pair=transport?stats.get(transport.selectedCandidatePairId):reports.find(r=>r.type==="candidate-pair"&&r.nominated&&r.state==="succeeded");
          peers.push({remotePort:pair?stats.get(pair.remoteCandidateId)?.port:null,audio:reports.filter(r=>r.type==="inbound-rtp"&&r.kind==="audio").map(r=>({packets:r.packetsReceived,bytes:r.bytesReceived,samples:r.totalSamplesReceived}))});
        }
        return peers;
      });
      if(mode==="webrtc"&&(result.peers.length!==4||result.peers.some(p=>p.remotePort!==udpPort||!p.audio.some(a=>a.packets>0))))throw Error("Four audio peers did not share the configured UDP port");
      for(const alias of ["aac","pcmu"]){
        await page.locator(`[data-camera="${alias}"] .camera-sound`).click();
        const audible=(await snapshot(page)).filter(v=>!v.muted).map(v=>v.id);
        if(audible.length!==1||audible[0]!==alias)throw Error("Camera sound focus did not mute other tiles");
      }
      result.singleAudibleCamera=true;
      await page.screenshot({path:path.join(output,`${mode}.png`),fullPage:true,mask:[page.locator("video")],maskColor:"#111719"});
      await page.locator("#disconnect").click();
      if(await page.locator(".camera-tile").count())throw Error("Wall cleanup left camera players");
      result.passed=result.errors.length===0;
    }catch(error){result.error=error.message;await page.screenshot({path:path.join(output,`${mode}-failure.png`),fullPage:true,mask:[page.locator("video")]}).catch(()=>{});}
    finally {await context.close();console.log(JSON.stringify({mode,passed:result.passed,error:result.error,sourceInputs:result.linuxAfter?.sourceInputs,mediaUdpPorts:result.linuxAfter?.mediaUdpPorts,mdnsSockets:result.linuxAfter?.mdnsSockets}));}
  }
}finally{await browser.close();await writeFile(path.join(output,"result.json"),JSON.stringify({passed:results.every(r=>r.passed),aliases,results},null,2));}
if(!results.every(r=>r.passed))process.exitCode=1;
