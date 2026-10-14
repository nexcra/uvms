import {chromium} from "playwright";
import {mkdir,writeFile} from "node:fs/promises";
const browser=await chromium.launch({executablePath:"C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",headless:true});
const page=await browser.newPage({viewport:{width:1440,height:900}});
const errors=[];page.on("pageerror",e=>errors.push(e.message));
const results=[];
try {
  await page.goto("http://127.0.0.1:8080");
  await page.waitForFunction(() => !document.querySelector("#disconnect").disabled);
  await page.locator("#disconnect").click();
  await page.locator("#load-streams").click();
  await page.waitForFunction(()=>!document.querySelector("#connect").disabled);
  await page.locator('button[data-layout="4"]').click();
  await page.locator('#camera-choices input[value="camera_copy"]').uncheck();
    if (await page.locator('#camera-choices input[value="camera_live"]').count()) await page.locator('#camera-choices input[value="camera_live"]').uncheck();
  await page.locator("#mode").selectOption("mse");
  await page.locator("#connect").click();
  await page.waitForFunction(()=>document.querySelectorAll('.camera-tile[data-state="live"]').length===2,null,{timeout:30000});
  for(const width of [1440,1100,900,390]){
    await page.setViewportSize({width,height:900});
    const state=await page.evaluate(()=>({width:innerWidth,scroll:document.documentElement.scrollWidth,labels:[...document.querySelectorAll(".camera-tile")].every(e=>e.getBoundingClientRect().width>0)}));
    if(state.scroll>width+1)throw Error("horizontal overflow "+width);
    results.push(state);
  }
  await page.setViewportSize({width:1440,height:1000});
  await page.locator('button[data-layout="1"]').click();
  if(await page.locator(".camera-tile").count())throw Error("single transition leaked wall");
  await page.locator("#stream").selectOption("camera");
  await page.locator("#connect").click();
  await page.waitForFunction(()=>document.querySelector("#video").videoWidth===640,null,{timeout:30000});
  await page.locator("#disconnect").click();
  await page.locator('button[data-layout="4"]').click();
  await page.locator("#connect").click();
  await page.waitForFunction(()=>document.querySelectorAll('.camera-tile[data-state="live"]').length===2,null,{timeout:30000});
  await page.locator('#camera-choices input[value="camera_two"]').uncheck();
  if(await page.locator(".camera-tile").count()!==1)throw Error("unselect cleanup");
  await page.locator('#camera-choices input[value="camera_two"]').check();
  await page.waitForFunction(()=>document.querySelectorAll('.camera-tile[data-state="live"]').length===2,null,{timeout:30000});
  await page.screenshot({path:"test-results/camera-wall/final.png",fullPage:true,mask:[page.locator("video")],maskColor:"#111719"});
  await page.locator("#disconnect").click();
  if(await page.locator("#camera-wall > *").count()!==4)throw Error("idle placeholders");
  await writeFile("test-results/camera-wall/layout.json",JSON.stringify({passed:errors.length===0,errors,results,singleAndMultiTransition:true,liveSelection:true},null,2));
} finally{await browser.close();}
