import { fileURLToPath } from "node:url";
import { browserPath } from "./browser-path.mjs";
// Bounded Windows Edge viewers against the WSL service, without credentials.
import { chromium } from "playwright";
import { spawn, execFileSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
const baseUrl = process.env.UVMS_BENCH_URL || "http://127.0.0.1:8088";
const output = process.env.UVMS_BENCH_OUTPUT || "test-results/performance";
const service = process.env.UVMS_BENCH_SERVICE || "uvms-java";
const warmup = Number(process.env.UVMS_BENCH_WARMUP || 2);
const modes = (process.env.UVMS_BENCH_MODES || "webrtc,mse").split(",");
const seconds = Number(process.env.UVMS_BENCH_SECONDS || 20);
const width = Number(process.env.UVMS_BENCH_WIDTH || 1920);
const height = Number(process.env.UVMS_BENCH_HEIGHT || 1080);
const counts = (process.env.UVMS_BENCH_COUNTS || "1,4,9,16").split(",").map(Number);
if (!(seconds >= 5 && seconds <= 120) || counts.some(n => !Number.isInteger(n) || n < 1 || n > 24)) throw Error("Invalid bounded test parameters");
const samples = [];
const samplePath = fileURLToPath(new URL('./sample-service.py', import.meta.url));
const distro = process.env.UVMS_WSL_DISTRO || 'Ubuntu-24.04';
const sampleEnv = {...process.env, UVMS_BENCH_SERVICE:service, UVMS_BENCH_SOURCE_PORT:process.env.UVMS_BENCH_SOURCE_PORT || '554'};
const sampler = process.platform === 'win32'
  ? spawn('wsl', ['-d',distro,'-u','root','--exec','env',`UVMS_BENCH_SERVICE=${service}`,`UVMS_BENCH_SOURCE_PORT=${sampleEnv.UVMS_BENCH_SOURCE_PORT}`,'python3','-u',execFileSync('wsl',['-d',distro,'--exec','wslpath','-a',samplePath],{encoding:'utf8'}).trim()], {stdio:['ignore','pipe','pipe']})
  : spawn('python3',['-u',samplePath],{env:sampleEnv,stdio:['ignore','pipe','pipe']});
let pending="";
sampler.stdout.on("data", data => {pending+=data;let end;while((end=pending.indexOf("\n"))>=0){const line=pending.slice(0,end);pending=pending.slice(end+1);try{samples.push(JSON.parse(line));}catch{}}});
const browser = await chromium.launch({executablePath: browserPath,headless:true,args:["--disable-background-timer-throttling","--disable-renderer-backgrounding","--disable-backgrounding-occluded-windows"]});
const results = {baseUrl,service,warmup,scope:process.env.UVMS_BENCH_SCOPE || "One real 1920x1080 H264 ~10fps camera, multiple independent browser viewers; recording disabled for this camera",seconds,stages:[],startedAt:new Date().toISOString()};
const snapshot = page => page.evaluate(async () => {
  const video = document.querySelector("#video"), quality = video.getVideoPlaybackQuality();
  const data = {width:video.videoWidth,height:video.videoHeight,time:performance.now()/1000,frames:quality.totalVideoFrames-quality.droppedVideoFrames,dropped:quality.droppedVideoFrames,bytes:session?.receivedBytes||0};
  if(session?.pc) {
    for(const item of (await session.pc.getStats()).values()) if(item.type==="inbound-rtp"&&(item.kind==="video"||item.mediaType==="video")) {
      Object.assign(data,{frames:item.framesDecoded||0,bytes:item.bytesReceived||0,packetsLost:item.packetsLost||0,jitter:item.jitter||0});
    }
  }
  return data;
});
const mean = values => values.reduce((a,b)=>a+b,0)/values.length;
try {
 for(const mode of modes) for(const count of counts) {
  const context = await browser.newContext({viewport:{width:960,height:720}});
  const pages=[],stage={mode,viewers:count,errors:[],passed:false};results.stages.push(stage);
  try {
   for(let batch=0;batch<count;batch+=4) await Promise.all(Array.from({length:Math.min(4,count-batch)},async()=>{
    const page=await context.newPage();pages.push(page);page.on("pageerror",e=>stage.errors.push(e.message));
    const started=Date.now();
    await page.goto(baseUrl);
    await page.waitForFunction(()=>!document.querySelector("#disconnect").disabled);
    if(mode==="mse") {await page.locator("#disconnect").click();await page.locator("#mode").selectOption(mode);await page.locator("#connect").click();}
    await page.waitForFunction(w=>document.querySelector("#video").videoWidth===w&&Number(document.querySelector("#frames").textContent.replaceAll(",",""))>=10,width,{timeout:45000});
    page.startupMs=Date.now()-started;
   }));
   await new Promise(r=>setTimeout(r,warmup*1000));
   const before=await Promise.all(pages.map(snapshot)),start=Date.now()/1000;
   await new Promise(r=>setTimeout(r,seconds*1000));
   const after=await Promise.all(pages.map(snapshot)),end=Date.now()/1000;
   const metrics=samples.filter(s=>s.time>=start&&s.time<=end&&s.cpu_percent_one_core!==undefined);
   if(!metrics.length)throw Error("No service resource samples");
   stage.clients=after.map((a,i)=>({fps:(a.frames-before[i].frames)/(a.time-before[i].time),mbps:8*(a.bytes-before[i].bytes)/(a.time-before[i].time)/1e6,frames:a.frames-before[i].frames,dropped:a.dropped-before[i].dropped,packetsLost:a.packetsLost===undefined?undefined:a.packetsLost-(before[i].packetsLost||0),width:a.width,height:a.height}));
   stage.meanFps=mean(stage.clients.map(c=>c.fps));stage.minFps=Math.min(...stage.clients.map(c=>c.fps));
   stage.totalMbps=stage.clients.reduce((s,c)=>s+c.mbps,0);
   stage.cpuMeanOneCorePercent=mean(metrics.map(m=>m.cpu_percent_one_core));
   stage.cpuPeakOneCorePercent=Math.max(...metrics.map(m=>m.cpu_percent_one_core));
   stage.rssPeakMiB=Math.max(...metrics.map(m=>m.rss_mib));
   stage.cameraInputsMax=Math.max(...metrics.map(m=>m.camera_inputs));
   stage.sourceInputsMax=Math.max(...metrics.map(m=>m.source_inputs));
   stage.udpSocketsMax=Math.max(...metrics.map(m=>m.udp_ports.length));
   stage.udpPorts=[...new Set(metrics.flatMap(m=>m.udp_ports))];
   stage.logicalCpus=metrics[0].logical_cpus;
   stage.startupMaxMs=Math.max(...pages.map(p=>p.startupMs));
   stage.passed=!stage.errors.length&&stage.clients.every(c=>c.frames>0&&c.width===width&&c.height===height);
  }catch(e){stage.error=e.message;}
  finally {await context.close();console.log(JSON.stringify(stage));await mkdir(output,{recursive:true});await writeFile(output+"/viewers.json",JSON.stringify(results,null,2));}
  if(!stage.passed)break;
 }
}finally {
 await browser.close();sampler.kill();
 results.finishedAt=new Date().toISOString();
 results.passed=results.stages.length===counts.length*modes.length&&results.stages.every(s=>s.passed);
 await mkdir(output,{recursive:true});
 await writeFile(output+"/viewers.json",JSON.stringify(results,null,2));
 await writeFile(output+"/service-samples.json",JSON.stringify(samples,null,2));
}
if(!results.passed)process.exitCode=1;
