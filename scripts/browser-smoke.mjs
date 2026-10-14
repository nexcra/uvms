import { chromium } from "playwright";
import { mkdir, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";

const baseUrl = process.env.UVMS_TEST_URL || "http://127.0.0.1:8080";
const stream = process.env.UVMS_TEST_STREAM;
const mode = process.env.UVMS_TEST_MODE || "webrtc";
const output = path.resolve(process.env.UVMS_TEST_OUTPUT || "test-results");
const edge = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const executablePath = process.env.UVMS_BROWSER_PATH || (existsSync(edge) ? edge : undefined);
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ executablePath, headless: true });
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
const result = { url: baseUrl, mode, startedAt: new Date().toISOString(), passed: false, errors };
try {
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.locator('#stream option').first().waitFor({state: 'attached'});
  // A failing default camera can disconnect before this check. Select the
  // requested fixture even when that unrelated default input is unavailable.
  await page.locator('#disconnect').evaluate(button => { if (!button.disabled) button.click(); });
  await page.locator("#load-streams").click();
  await page.waitForFunction(() => !document.querySelector("#connect").disabled, null, { timeout: 15000 });
  if (stream) await page.locator("#stream").selectOption(stream);
  await page.locator("#mode").selectOption(mode);
  result.stream = await page.locator("#stream").inputValue();
  await page.locator("#connect").click();
  await page.waitForFunction(() => {
    const video = document.querySelector("#video");
    const decoded = Number(document.querySelector("#frames").textContent.replaceAll(",", ""));
    return video.videoWidth > 0 && video.videoHeight > 0 && decoded > 30;
  }, null, { timeout: 60000 });
  result.video = await page.locator("#video").evaluate((video) => ({ width: video.videoWidth, height: video.videoHeight, currentTime: video.currentTime, readyState: video.readyState, paused: video.paused, playbackQuality: video.getVideoPlaybackQuality?.().totalVideoFrames }));
  result.stats = await page.evaluate(() => Object.fromEntries(["frames", "bytes", "fps", "resolution", "status-label", "freshness"].map((id) => [id, document.getElementById(id).textContent])));
  result.storageKeys = await page.evaluate(() => ({ local: Object.keys(localStorage), session: Object.keys(sessionStorage) }));
  // Mask camera pixels in screenshots of real inputs.
  await page.screenshot({ path: path.join(output, "player.png"), fullPage: true, mask: [page.locator("video")], maskColor: "#111819" });
  await page.locator("#disconnect").click();
  await page.waitForFunction(() => document.querySelector("#video").srcObject === null && document.querySelector("#disconnect").disabled);
  result.disconnectPassed = true;
  await page.locator("#reconnect").click();
  await page.waitForFunction(() => document.querySelector("#video").videoWidth > 0 && Number(document.querySelector("#frames").textContent.replaceAll(",", "")) > 30, null, { timeout: 60000 });
  result.reconnectPassed = true;
  await page.locator("#disconnect").click();
  result.passed = errors.length === 0;
} catch (error) {
  result.error = error.message;
  result.uiError = await page.locator("#error").textContent().catch(() => "");
  await page.screenshot({ path: path.join(output, "player-failure.png"), fullPage: true, mask: [page.locator("video")], maskColor: "#111819" }).catch(() => {});
} finally {
  result.finishedAt = new Date().toISOString();
  await browser.close();
  await writeFile(path.join(output, "browser.json"), JSON.stringify(result, null, 2));
}
console.log(JSON.stringify(result, null, 2));
if (!result.passed) process.exitCode = 1;
