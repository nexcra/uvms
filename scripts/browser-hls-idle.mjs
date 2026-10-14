// Verify live DVR demand stops with its player; use an isolated synthetic server.
import { chromium } from "playwright";
import { existsSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
const base = process.env.UVMS_TEST_URL;
if (!base || ["8080", "8088"].includes(new URL(base).port)) throw Error("Use an isolated test server");
const output = process.env.UVMS_TEST_OUTPUT || "test-results/hls-idle/browser-lifecycle";
await mkdir(output, {recursive:true});
const edge = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const browser = await chromium.launch({headless:true,executablePath:process.env.UVMS_BROWSER_PATH || (existsSync(edge)?edge:undefined)});
const page = await browser.newPage();
const result = {passed:false,errors:[]};
page.on("pageerror", e => result.errors.push(e.message));
const state = async () => (await page.request.get(base + "/api/hls/aac")).json();
try {
  await page.goto(base);
  await page.waitForFunction(() => document.querySelector("#video").videoWidth > 0 && Number(document.querySelector("#frames").textContent.replaceAll(",", "")) > 15);
  // Hold longer than this fixture's idle timeout. Status requests cannot renew it.
  const idle = (await state()).idle_seconds;
  if (!(idle >= 15 && idle <= 20)) throw Error("Use the short isolated idle timeout");
  for (let i=0;i<idle+3;i++) { await page.waitForTimeout(1000); await state(); }
  let live = await state();
  if (!live.running || !live.ready || !live.bytes) throw Error("Live WebRTC lost its rewind cache");
  result.liveRewindMaintained = true;
  await page.locator("#disconnect").click();
  for (let i=0;i<idle+3;i++) { await page.waitForTimeout(1000); await state(); }
  let stopped = await state();
  if (stopped.running || stopped.bytes) throw Error("Disconnected player renewed its HLS worker");
  result.disconnectReleasesCache = true;
  await page.locator("#reconnect").click();
  await page.waitForFunction(() => document.querySelector("#video").videoWidth > 0 && Number(document.querySelector("#frames").textContent.replaceAll(",", "")) > 15);
  for(let i=0;i<12 && !(await state()).ready;i++) await page.waitForTimeout(1000);
  if (!(await state()).ready) throw Error("Reconnected live playback did not rebuild DVR");
  result.reconnectRebuildsCache = true;
  result.passed = result.errors.length === 0;
} catch(e) {result.error=e.message;}
finally {await browser.close(); await writeFile(output + "/result.json",JSON.stringify(result,null,2)); console.log(JSON.stringify(result));}
if(!result.passed)process.exitCode=1;
