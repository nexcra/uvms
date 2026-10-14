import { chromium } from "playwright";
import { mkdir, writeFile } from "node:fs/promises";
const browser = await chromium.launch({executablePath: "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe", headless: true});
const page = await browser.newPage({viewport: {width:1440,height:1000}});
const result = {passed:false,errors:[]};
const output = process.env.UVMS_TEST_OUTPUT || 'test-results/local-access';
page.on("pageerror", e => result.errors.push(e.message));
const waitLive = () => page.waitForFunction(() => document.querySelector("#video").videoWidth === 1920 && Number(document.querySelector("#frames").textContent.replaceAll(",", "")) > 20, null, {timeout:45000});
try {
  await page.goto(process.env.UVMS_TEST_URL || "http://127.0.0.1:8080");
  await waitLive();
  result.automaticPlayback = await page.locator("#stream").inputValue() === "camera_live";
  result.noTokenPrompt = await page.locator("#token").count() === 0;
  await page.reload();
  await waitLive();
  result.refreshPlayback = true;
  await page.locator("#disconnect").click();
  await page.locator("#mode").selectOption("mse");
  await page.locator("#connect").click();
  await waitLive();
  result.msePlayback = true;
  await page.locator("#refresh-recordings").click();
  await page.waitForFunction(() => document.querySelector("#recording-summary").textContent.includes("MB"));
  result.recordingsAccessible = true;
  await page.locator('button[data-layout="4"]').click();
  for (const id of ["camera_copy","camera_two"]) {
    const choice = page.locator(`#camera-choices input[value="${id}"]`);
    if (await choice.count()) await choice.uncheck();
  }
  await page.locator('#camera-choices input[value="camera_live"]').check();
  await page.locator("#connect").click();
  await page.waitForFunction(() => document.querySelectorAll('.camera-tile[data-state="live"]').length === 2, null, {timeout:45000});
  result.multiCamera = true;
  // Preserve layout evidence without capturing the private camera picture.
  await mkdir(output, {recursive:true});
  await page.screenshot({path:output+"/layout.png", fullPage:true, mask:[page.locator("video")],maskColor:"#111719"});
  result.passed = result.automaticPlayback && result.noTokenPrompt && !result.errors.length;
} catch(e) {result.error=e.message;}
finally {
  await browser.close();
  await mkdir(output, {recursive:true});
  await writeFile(output+"/result.json",JSON.stringify(result,null,2));
  console.log(JSON.stringify(result));
}
if (!result.passed) process.exitCode=1;
