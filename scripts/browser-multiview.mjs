import { chromium } from "playwright";
import { mkdir, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";

const baseUrl = process.env.UVMS_TEST_URL || "http://127.0.0.1:8080";
const stream = process.env.UVMS_TEST_STREAM || "camera";
const mode = process.env.UVMS_TEST_MODE || "webrtc";
const output = path.resolve(process.env.UVMS_TEST_OUTPUT || "test-results");
const edge = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const executablePath = process.env.UVMS_BROWSER_PATH || (existsSync(edge) ? edge : undefined);
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ executablePath, headless: true });
const contexts = await Promise.all([browser.newContext({ viewport: { width: 1440, height: 900 } }), browser.newContext({ viewport: { width: 1440, height: 900 } })]);
const pages = await Promise.all(contexts.map((context) => context.newPage()));
const errors = [];
pages.forEach((page, index) => {
  page.on("pageerror", (error) => errors.push({ viewer: index + 1, message: error.message }));
  page.on("console", (message) => { if (message.type() === "error") errors.push({ viewer: index + 1, message: message.text(), url: message.location().url }); });
});
const result = { url: baseUrl, stream, mode, startedAt: new Date().toISOString(), passed: false, errors };

const snapshot = (page) => page.evaluate(() => ({
  width: document.querySelector("#video").videoWidth,
  height: document.querySelector("#video").videoHeight,
  decodedFrames: Number(document.querySelector("#frames").textContent.replaceAll(",", "")),
  videoTime: document.querySelector("#video").currentTime,
  state: document.querySelector("#status").dataset.state,
  bufferedRanges: Array.from({ length: document.querySelector("#video").buffered.length }, (_, index) => ({ start: document.querySelector("#video").buffered.start(index), end: document.querySelector("#video").buffered.end(index) })),
}));

async function connectViewer(page) {
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.waitForFunction(() => !document.querySelector("#disconnect").disabled);
  await page.locator("#disconnect").click();
  await page.locator("#load-streams").click();
  await page.waitForFunction(() => !document.querySelector("#connect").disabled, null, { timeout: 15000 });
  await page.locator("#stream").selectOption(stream);
  await page.locator("#mode").selectOption(mode);
  await page.locator("#connect").click();
  await page.waitForFunction(() => {
    const video = document.querySelector("#video");
    const frames = Number(document.querySelector("#frames").textContent.replaceAll(",", ""));
    return video.videoWidth > 0 && video.videoHeight > 0 && frames > 30;
  }, null, { timeout: 60000 });
}

try {
  await Promise.all(pages.map(connectViewer));
  result.bothConnected = await Promise.all(pages.map(snapshot));
  if (mode === "mse") {
    result.sustainedSeconds = 25;
    await pages[0].waitForTimeout(result.sustainedSeconds * 1000);
    result.sustainedViewers = await Promise.all(pages.map(snapshot));
    result.sustainedViewers.forEach((viewer, index) => {
      viewer.framesAdded = viewer.decodedFrames - result.bothConnected[index].decodedFrames;
      viewer.bufferedSeconds = viewer.bufferedRanges.reduce((total, range) => total + range.end - range.start, 0);
      viewer.bufferedSpanSeconds = viewer.bufferedRanges.length ? viewer.bufferedRanges.at(-1).end - viewer.bufferedRanges[0].start : 0;
      if (viewer.framesAdded <= 30 || viewer.state !== "live") throw new Error(`Viewer ${index + 1} stopped decoding during sustained MSE playback.`);
      if (!viewer.bufferedRanges.length || viewer.bufferedSpanSeconds > 20) throw new Error(`Viewer ${index + 1} has an unbounded or empty MSE buffer (${viewer.bufferedSpanSeconds}s).`);
    });
  }
  await pages[0].screenshot({ path: path.join(output, "player.png"), fullPage: true, mask: [pages[0].locator("video")], maskColor: "#111819" });
  const secondBefore = (await snapshot(pages[1])).decodedFrames;
  await pages[0].locator("#disconnect").click();
  await pages[0].waitForFunction(() => document.querySelector("#video").srcObject === null && document.querySelector("#disconnect").disabled);
  result.firstDisconnected = true;
  await pages[1].waitForFunction((before) => {
    const frames = Number(document.querySelector("#frames").textContent.replaceAll(",", ""));
    return frames > before + 30 && document.querySelector("#status").dataset.state === "live";
  }, secondBefore, { timeout: 15000 });
  result.remainingViewer = await snapshot(pages[1]);
  result.framesAfterFirstDisconnect = result.remainingViewer.decodedFrames - secondBefore;
  await pages[1].locator("#disconnect").click();
  await pages[1].waitForFunction(() => document.querySelector("#video").srcObject === null && document.querySelector("#disconnect").disabled);
  result.bothDisconnected = true;
  result.passed = errors.length === 0;
} catch (error) {
  result.error = error.message;
  result.uiErrors = await Promise.all(pages.map((page) => page.locator("#error").textContent().catch(() => "")));
} finally {
  for (const page of pages) {
    await page.locator("#disconnect").click({ timeout: 1000 }).catch(() => {});
  }
  await browser.close();
  result.finishedAt = new Date().toISOString();
  await writeFile(path.join(output, "multiview.json"), JSON.stringify(result, null, 2));
}
console.log(JSON.stringify(result, null, 2));
if (!result.passed) process.exitCode = 1;
