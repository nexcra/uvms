import {chromium} from "playwright";
import {execFileSync} from "node:child_process";
import {mkdir,writeFile} from "node:fs/promises";
const output="test-results/camera-wall";
await mkdir(output,{recursive:true});
const browser=await chromium.launch({executablePath:"C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",headless:true});
const results=[];
const control=action=>execFileSync("wsl",["-d","Ubuntu-24.04","-u","root","--exec","systemctl",action,"uvms-demo-source-two"],{timeout:15000});
const snapshot=page=>page.evaluate(()=>[...document.querySelectorAll(".camera-tile")].map(e=>({id:e.dataset.camera,state:e.dataset.state,frames:Number(e.dataset.frames),time:e.querySelector("video").currentTime,recording:e.querySelector(".camera-rec").textContent,width:e.querySelector("video").videoWidth})));
try {
 for (const mode of ["webrtc","mse"]) {
  const page=await browser.newPage({viewport:{width:1440,height:1000}});
  const result={mode,passed:false,errors:[],sockets:[]};results.push(result);
  page.on("pageerror",e=>result.errors.push(e.message));
  page.on("websocket",ws=>result.sockets.push(new URL(ws.url()).pathname));
  try {
    await page.goto("http://127.0.0.1:8080");
    await page.waitForFunction(() => !document.querySelector("#disconnect").disabled);
    await page.locator("#disconnect").click();
    await page.locator("#load-streams").click();
    await page.waitForSelector('#camera-choices input[value="camera_two"]',{state:"attached"});
    await page.locator('button[data-layout="4"]').click();
    await page.locator('#camera-choices input[value="camera_copy"]').uncheck();
    if (await page.locator('#camera-choices input[value="camera_live"]').count()) await page.locator('#camera-choices input[value="camera_live"]').uncheck();
    result.sockets.length = 0;
    await page.locator("#mode").selectOption(mode);
    await page.locator("#connect").click();
    await page.waitForFunction(()=>document.querySelectorAll('.camera-tile[data-state="live"]').length===2,null,{timeout:40000});
    result.before=await snapshot(page);
    await page.screenshot({path:output+"/"+mode+".png",fullPage:true,mask:[page.locator("video")]});
    for(const count of [9,16,4]){
      await page.locator('button[data-layout="'+count+'"]').click();
      if(await page.locator("#camera-wall > *").count()!==count)throw Error("layout slot count");
    }
    result.layoutNoReconnect=result.sockets.length===2;
    await page.locator('[data-camera="camera"] .camera-expand').click();
    await page.waitForTimeout(14000);
    await page.locator('[data-camera="camera"] .camera-expand').click();
    result.focusKeptConnections=result.sockets.length===2;
    const beforeHealthy=(await snapshot(page)).find(c=>c.id==="camera").frames;
    control("stop");
    await page.waitForFunction(()=>document.querySelector('[data-camera="camera_two"]').dataset.state!=="live",null,{timeout:25000});
    await page.waitForFunction(before=>Number(document.querySelector('[data-camera="camera"]').dataset.frames)>before+10,beforeHealthy,{timeout:15000});
    result.duringOutage=await snapshot(page);
    control("start");
    await page.waitForFunction(()=>document.querySelector('[data-camera="camera_two"]').dataset.state==="live",null,{timeout:40000});
    result.recovered=await snapshot(page);
    const healthySockets=result.sockets.filter(p=>p.endsWith("/camera")).length;
    if(healthySockets!==1)throw Error("unaffected camera was reconnected");
    await page.locator('[data-camera="camera_two"] .camera-retry').click();
    await page.waitForFunction(()=>document.querySelectorAll('.camera-tile[data-state="live"]').length===2,null,{timeout:35000});
    await page.locator("#disconnect").click();
    if(await page.locator(".camera-tile").count())throw Error("wall cleanup failed");
    result.passed=result.errors.length===0&&result.layoutNoReconnect&&result.focusKeptConnections;
  } catch(error){result.error=error.message;await page.screenshot({path:output+"/"+mode+"-failure.png",fullPage:true}).catch(()=>{});}
  finally {control("start");await page.close();console.log(JSON.stringify(result));}
 }
} finally {
 await browser.close();
 await writeFile(output+"/result.json",JSON.stringify(results,null,2));
}
if(!results.every(r=>r.passed))process.exitCode=1;
