import {chromium} from "playwright";
import {mkdir,writeFile} from "node:fs/promises";
const output="test-results/native-systemd";
await mkdir(output,{recursive:true});
const browser=await chromium.launch({executablePath:"C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",headless:true});
const page=await browser.newPage({viewport:{width:1440,height:1000},acceptDownloads:true});
const errors=[];page.on("pageerror",e=>errors.push(e.message));
try {
  await page.goto("http://127.0.0.1:8080");
  await page.waitForFunction(() => !document.querySelector("#disconnect").disabled);
  await page.locator("#disconnect").click();
  await page.locator("#load-streams").click();
  await page.waitForFunction(()=>!document.querySelector("#connect").disabled);
  await page.locator("#mode").selectOption("mse");
  await page.locator("#connect").click();
  await page.waitForFunction(()=>document.querySelector("#video").videoWidth===640,{},{timeout:30000});
  await page.locator("#refresh-recordings").click();
  await page.waitForSelector("#recordings a");
  const name=await page.locator("#recordings a").first().textContent();
  const [download]=await Promise.all([page.waitForEvent("download"),page.locator("#recordings a").first().click()]);
  const path=output+"/downloaded.mp4";await download.saveAs(path);
  await page.screenshot({path:output+"/recordings.png",fullPage:true,mask:[page.locator("video")]});
  await page.locator("#disconnect").click();
  await writeFile(output+"/browser.json",JSON.stringify({passed:errors.length===0,errors,name,download:path,summary:await page.locator("#recording-summary").textContent()},null,2));
} finally {await browser.close();}
