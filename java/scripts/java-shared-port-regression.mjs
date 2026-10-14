import { browserPath } from "./browser-path.mjs";
// Real browser ICE/DTLS/SRTP across two aliases, peer removal and reconnect.
import { chromium } from 'playwright';
import { mkdir, writeFile } from 'node:fs/promises';
const base = process.env.UVMS_TEST_URL || 'http://127.0.0.1:8090';
const output = process.env.UVMS_TEST_OUTPUT || 'test-results/java-optimization/shared-port';
const browser = await chromium.launch({executablePath: browserPath,headless:true,args:['--disable-background-timer-throttling','--disable-renderer-backgrounding']});
const context = await browser.newContext();
const result = {passed:false, viewers:16, aliases:['camera','camera_two'], errors:[]};
async function open(alias) {
  const page = await context.newPage();
  page.on('pageerror',e=>result.errors.push(e.message));
  await page.goto(base);
  await page.locator('#stream option[value="'+alias+'"]').waitFor({state:'attached'});
  // The unavailable default camera may finish failing between checking and clicking.
  await page.locator('#disconnect').evaluate(button => { if (!button.disabled) button.click(); });
  await page.locator('#stream').selectOption(alias);
  await page.locator('#connect').click();
  await page.waitForFunction(()=>document.querySelector('#video').videoWidth>0&&Number(document.querySelector('#frames').textContent.replaceAll(',',''))>10,null,{timeout:45000});
  return page;
}
async function stats(page) {
  return page.evaluate(async()=>{
    const all=await session.pc.getStats();
    const pair=[...all.values()].find(s=>s.type==='candidate-pair'&&s.state==='succeeded'&&s.nominated);
    const candidate=pair&&all.get(pair.remoteCandidateId);
    const video=[...all.values()].find(s=>s.type==='inbound-rtp'&&s.kind==='video');
    return {port:candidate?.port,frames:video?.framesDecoded||0,lost:video?.packetsLost||0};
  });
}
try {
  const pages=[];
  for(let i=0;i<16;i+=4) pages.push(...await Promise.all(Array.from({length:4},(_,j)=>open((i+j)%2?'camera_two':'camera'))));
  result.initial=await Promise.all(pages.map(stats));
  const ports=[...new Set(result.initial.map(s=>s.port))];
  if(ports.length!==1||!ports[0]) throw Error('Viewers did not share one selected remote UDP port');
  result.port=ports[0];
  await Promise.all(pages.slice(0,8).map(p=>p.close()));
  const survivors=pages.slice(8);
  const before=await Promise.all(survivors.map(stats));
  await new Promise(r=>setTimeout(r,4000));
  result.survivors=await Promise.all(survivors.map(stats));
  if(result.survivors.some((s,i)=>s.frames<=before[i].frames)) throw Error('Closing peers interrupted surviving viewers');
  const replacements=[];
  for(let i=0;i<8;i+=4) replacements.push(...await Promise.all(Array.from({length:4},(_,j)=>open(j%2?'camera_two':'camera'))));
  result.reconnected=await Promise.all(replacements.map(stats));
  if(result.reconnected.some(s=>s.port!==result.port||s.frames<1)) throw Error('Replacement viewers failed on shared port');
  result.passed=result.errors.length===0;
} catch(e) { result.error=e.message; }
finally {
  await browser.close();
  await mkdir(output,{recursive:true});
  await writeFile(output+'/result.json',JSON.stringify(result,null,2));
  console.log(JSON.stringify(result));
}
if(!result.passed)process.exitCode=1;
