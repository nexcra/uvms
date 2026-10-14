// Rust-only, two-camera WebRTC fan-out, peer removal and late-join regression.
import { chromium } from "playwright";
import { mkdir, writeFile } from "node:fs/promises";
const base = process.env.UVMS_TEST_URL;
if(!base || ["8080","8088"].includes(new URL(base).port)) throw Error("Use an isolated Rust fixture with camera and camera_two");
const health = await (await fetch(base+"/healthz")).json();
if(health.media_engine!=="rust-native") throw Error("Expected Rust fixture");
const output = process.env.UVMS_TEST_OUTPUT || "test-results/rtp-sharing/peer-isolation";
const browser = await chromium.launch({executablePath:process.env.UVMS_BROWSER_PATH || "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",headless:true,args:["--disable-background-timer-throttling","--disable-renderer-backgrounding"]});
const context = await browser.newContext();
const result = {passed:false,health,viewers:16,aliases:["camera","camera_two"],errors:[],statsRetries:[]};
async function open(alias) {
  const page = await context.newPage();
  page.on("pageerror",error=>result.errors.push(error.message));
  await page.goto(base);
  await page.locator(`#stream option[value="${alias}"]`).waitFor({state:"attached"});
  await page.locator("#disconnect").evaluate(button=>{if(!button.disabled)button.click();});
  await page.locator("#stream").selectOption(alias);
  await page.locator("#connect").click();
  await page.waitForFunction(()=>document.querySelector("#video").videoWidth>0 && Number(document.querySelector("#frames").textContent.replaceAll(",",""))>10,null,{timeout:45000});
  return page;
}
async function stats(page) {
  for(let attempt=0;attempt<10;attempt++) {
    const snapshot = await page.evaluate(async()=>{
      const pc = session.pc, all = await pc.getStats(), values=[...all.values()];
      const video = values.find(s=>s.type==="inbound-rtp" && s.kind==="video");
      const transport = video && all.get(video.transportId);
      // Follow the selected transport, not an arbitrary nominated candidate.
      // https://www.w3.org/TR/webrtc-stats/#dom-rtctransportstats-selectedcandidatepairid
      const pair = all.get(transport?.selectedCandidatePairId) || values.find(s=>s.type==="candidate-pair" && s.state==="succeeded" && s.nominated);
      const candidate = pair && all.get(pair.remoteCandidateId);
      return {state:pc.connectionState,port:candidate?.port,frames:video?.framesDecoded||0,lost:video?.packetsLost||0,
        transportId:video?.transportId,selectedPair:transport?.selectedCandidatePairId,
        pairState:pair?.state,candidateId:pair?.remoteCandidateId};
    });
    if(snapshot.port && snapshot.state==="connected") return snapshot;
    result.statsRetries.push({attempt,...snapshot});
    await page.waitForTimeout(500);
  }
  throw Error("Selected ICE transport statistics did not become available");
}
try {
  const pages=[];
  for(let i=0;i<16;i+=4) pages.push(...await Promise.all(Array.from({length:4},(_,j)=>open((i+j)%2?"camera_two":"camera"))));
  result.initial=await Promise.all(pages.map(stats));
  const ports=[...new Set(result.initial.map(s=>s.port))];
  if(ports.length!==1) throw Error("Viewers do not share one remote media UDP port");
  result.port=ports[0];
  await Promise.all(pages.slice(0,8).map(page=>page.close()));
  const survivors=pages.slice(8), before=await Promise.all(survivors.map(stats));
  await new Promise(resolve=>setTimeout(resolve,4000));
  result.survivors=await Promise.all(survivors.map(stats));
  if(result.survivors.some((s,i)=>s.frames<=before[i].frames)) throw Error("Peer removal stopped a surviving viewer");
  const replacements=[];
  for(let i=0;i<8;i+=4) replacements.push(...await Promise.all(Array.from({length:4},(_,j)=>open(j%2?"camera_two":"camera"))));
  result.reconnected=await Promise.all(replacements.map(stats));
  if(result.reconnected.some(s=>s.port!==result.port || !s.frames)) throw Error("Late viewers failed on shared UDP port");
  const allPages=[...survivors,...replacements], start=await Promise.all(allPages.map(stats));
  await new Promise(resolve=>setTimeout(resolve,4000));
  result.final=await Promise.all(allPages.map(stats));
  if(result.final.some((s,i)=>s.frames<=start[i].frames || s.port!==result.port)) throw Error("Playback stopped after replacements joined");
  result.passed=result.errors.length===0;
} catch(error) {result.error=error.message;}
finally {
  await browser.close();
  await mkdir(output,{recursive:true});
  await writeFile(output+"/result.json",JSON.stringify(result,null,2));
  console.log(JSON.stringify(result));
}
if(!result.passed) process.exitCode=1;
