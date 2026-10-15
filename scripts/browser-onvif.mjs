import { chromium } from "playwright";
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

// Only the isolated fixture from verify-onvif.py --serve is allowed: these
// clicks deliberately move its simulated PTZ. Never point this at real cameras.
const base = "http://127.0.0.1:8097", mock = "http://127.0.0.1:18690";
const output = path.resolve("test-results/onvif"); await mkdir(output,{recursive:true});
const browser = await chromium.launch({headless:true,executablePath:"C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe"});
const page = await browser.newPage({viewport:{width:1440,height:1000}});
const errors = [], report = {passed:false,errors};
page.on("pageerror",error=>errors.push(error.message));
const events = async () => (await (await page.request.get(mock)).json()).events;
try {
  await page.goto(base);
  await page.locator('#onvif-camera option[value="fixture"]').waitFor({state:"attached"});
  await page.locator("#disconnect").evaluate(b=>{if(!b.disabled)b.click();});
  await page.locator("#onvif-camera").selectOption("fixture");
  await page.waitForFunction(()=>document.querySelector("#onvif-url").value.includes("device?channel=1"));
  assert.equal(await page.locator('[data-ptz="up"]').isDisabled(),true);
  await page.locator("#onvif-inspect").click();
  await page.waitForFunction(()=>!document.querySelector('[data-ptz="up"]').disabled);
  assert.match(await page.locator("#onvif-device-info").textContent(),/UVMS test/);
  await page.locator("#onvif-profile").selectOption("fixed");
  await page.waitForFunction(()=>document.querySelector("#onvif-hint").textContent.includes("지원되는 PTZ 이동 기능이 없습니다"));
  assert.equal(await page.locator('[data-ptz="zoom-in"]').isDisabled(),true);
  await page.locator("#onvif-profile").selectOption("main");
  await page.waitForFunction(()=>!document.querySelector('[data-ptz="up"]').disabled);
  for (const direction of ["up","left","right","down","zoom-in","zoom-out"]) {
    const before=(await events()).length;
    await page.locator(`[data-ptz="${direction}"]`).click();
    await page.waitForFunction(()=>document.querySelector("#onvif-hint").textContent.includes("이동 후 정지 명령"));
    assert.deepEqual((await events()).slice(before).map(e=>e.operation),["ContinuousMove","Stop"]);
  }
  await page.locator("#ptz-stop").click();
  await page.waitForFunction(()=>document.querySelector("#onvif-hint").textContent==="정지 명령이 확인됐습니다.");
  await page.locator("#onvif-discover").evaluate(b=>b.closest("details").open=true);
  await page.locator("#onvif-discover").click();
  await page.locator("#onvif-devices button").waitFor();
  assert.equal(await page.locator("#onvif-devices article").count(),1);
  await page.locator("#onvif-devices button").click();
  await page.waitForFunction(()=>!document.querySelector('[data-ptz="up"]').disabled);
  await page.locator("#onvif-hint").scrollIntoViewIfNeeded();
  await page.screenshot({path:path.join(output,"desktop.png"),fullPage:true});
  for (const width of [930,390]) {
    await page.setViewportSize({width,height:844});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1),false,"overflow at "+width);
  }
  await page.locator("#onvif-camera").scrollIntoViewIfNeeded();
  await page.screenshot({path:path.join(output,"mobile.png"),fullPage:true});
  await page.locator("#onvif-camera").selectOption("hostile");
  await page.waitForFunction(()=>document.querySelector("#onvif-url").value.endsWith("/hostile"));
  await page.locator("#onvif-inspect").click();
  await page.waitForFunction(()=>document.querySelector("#onvif-state").textContent==="확인 필요");
  assert.equal(await page.locator('[data-ptz="up"]').isDisabled(),true);
  assert.deepEqual(errors,[]);
  report.passed=true;
  report.verified=["discovery selection","inspection and camera/profile switching","unsupported controls disabled","six directional/zoom actions each followed by Stop","explicit Stop","inspection failure keeps controls disabled","desktop/mobile no horizontal overflow","no page errors"];
} catch(error) {report.error=error.stack;await page.screenshot({path:path.join(output,"failure.png"),fullPage:true});}
finally {await browser.close();await writeFile(path.join(output,"browser.json"),JSON.stringify(report,null,2));}
console.log(JSON.stringify(report,null,2));if(!report.passed)process.exitCode=1;
