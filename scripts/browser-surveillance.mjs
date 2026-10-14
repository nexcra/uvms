import { chromium } from "playwright";
import { createServer } from "node:http";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";

// Default is an isolated UI contract fixture. --live requires an explicit test
// endpoint and camera; recording controls are restored even after failure.
const live = process.argv.includes("--live");
if (live && (!process.env.UVMS_TEST_URL || !process.env.UVMS_TEST_STREAM)) throw new Error("Live testing requires UVMS_TEST_URL and UVMS_TEST_STREAM for an isolated fixture.");
const stream = process.env.UVMS_TEST_STREAM || "test_camera";
const output = path.resolve(process.env.UVMS_TEST_OUTPUT || "test-results/surveillance-ui");
await mkdir(output, {recursive:true});
let server, base = process.env.UVMS_TEST_URL, control = {mode:"off",schedules:[],status:"off",active:false};
const changes = [], errors = [];
const now = Date.now();
if (!live) {
  server = createServer(async (req,res) => {
    const url = new URL(req.url,"http://localhost");
    const json = value => { res.setHeader("Content-Type","application/json"); res.end(JSON.stringify(value)); };
    if (url.pathname === "/api/streams") return json({default_stream:stream,streams:process.argv.includes("--umbra") ? [] : [{id:stream,record:false,recording:"off"}]});
    if (url.pathname.startsWith("/api/recording/")) {
      if (req.method === "POST") {
        let body = ""; for await (const chunk of req) body += chunk;
        const value = JSON.parse(body); changes.push(value);
        control = {...value,status:value.mode === "on" ? "recording" : value.mode === "auto" ? "scheduled" : "off",active:value.mode === "on"};
      }
      return json(control);
    }
    if (url.pathname === "/api/recordings") return json({used_bytes:123456,limit_bytes:10485760,files:[{stream,file:now+"-1.mp4",start_ms:now-30000,end_ms:now-20000,bytes:123456,estimated:false,url:"/api/recordings/"+stream+"/"+now+"-1.mp4"}]});
    if (url.pathname.startsWith("/api/hls/")) return json({ready:false,oldest_ms:null,newest_ms:null,window_seconds:300,bytes:0});
    if (url.pathname.startsWith("/webrtc_")) { res.statusCode=503; return res.end(); }
    const files = {"/":"index.html","/index.html":"index.html","/app.js":"app.js","/style.css":"style.css","/vendor/hls.min.js":"vendor/hls.min.js"};
    if (!files[url.pathname]) {res.statusCode=404;return res.end();}
    try {
      let data = await readFile(path.resolve("web",files[url.pathname]));
      if (process.argv.includes("--negative-focus") && url.pathname.endsWith(".css")) data = Buffer.from(data.toString().replace(/\.field:focus-within\s*\{[^}]+\}/,""));
      res.setHeader("Content-Type",url.pathname.endsWith(".js") ? "text/javascript" : url.pathname.endsWith(".css") ? "text/css" : "text/html"); res.end(data);
    }
    catch {res.statusCode=404;res.end();}
  });
  await new Promise(resolve => server.listen(0,"127.0.0.1",resolve));
  base = "http://127.0.0.1:"+server.address().port;
}
const edge = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
if (process.argv.includes("--umbra")) {
  const source = await readFile("C:/Users/crane/.codex/skills/umbra/scripts/verify-screen.js","utf8");
  // Chromium's native date/time editor focuses its internal segments. The host
  // input reports :focus-within, not :focus-visible, while our visible ring is
  // still required by the unchanged computed outline checks below.
  const adapted = source.replace("chromium.launch()","chromium.launch({executablePath:"+JSON.stringify(edge)+"})")
    .replace('e.matches(":focus-visible")', '(e.matches(":focus-visible") || (e.type === "datetime-local" && e.matches(":focus-within")))');
  const file = path.join(output,"verify-screen.cjs"); await writeFile(file,adapted);
  const code = await new Promise(resolve => { const child=spawn(process.execPath,[file,base],{cwd:output,stdio:"inherit"}); child.on("exit",resolve); });
  if(server) await new Promise(resolve=>server.close(resolve));
  process.exit(code || 0);
}
const browser = await chromium.launch({headless:true,executablePath:process.env.UVMS_BROWSER_PATH || (existsSync(edge)?edge:undefined)});
const page = await browser.newPage({viewport:{width:1440,height:1000}});
if (process.env.UVMS_FORCE_HLS_JS === "1") await page.addInitScript(() => {
  const original = HTMLMediaElement.prototype.canPlayType;
  HTMLMediaElement.prototype.canPlayType = function(type) {return /mpegurl/i.test(type)?"":original.call(this,type);};
});
page.on("pageerror",error=>errors.push(error.message));
const result = {kind:live?"live integration":"isolated UI contract",passed:false,errors};
let original;
try {
  await page.goto(base);
  await page.waitForFunction(() => document.querySelector("#record-camera").options.length > 0);
  await page.locator("#disconnect").evaluate(b=>{if(!b.disabled)b.click();});
  await page.locator("#record-camera").selectOption(stream);
  await page.waitForFunction(() => !document.querySelector('[data-record-mode="on"]').disabled);
  original = await (await page.request.get(base+"/api/recording/"+encodeURIComponent(stream))).json();
  const recordingStarted = Date.now();
  await page.locator('[data-record-mode="on"]').click();
  await page.waitForFunction(() => document.querySelector('[data-record-mode="on"]').getAttribute("aria-pressed")==="true");
  if (live) {
    // waitForFunction predicates are synchronous; a Promise itself would look
    // truthy before the recording exists. Poll the HTTP result explicitly.
    const deadline = Date.now()+30000;
    while (Date.now()<deadline) {
      const response = await page.request.get(base+"/api/recordings?stream="+encodeURIComponent(stream)+"&from="+(recordingStarted-1000));
      if(response.ok()) result.recordingCreated = (await response.json()).files.find(file=>file.stream===stream && file.start_ms>=recordingStarted-1000 && file.bytes>0);
      if(result.recordingCreated) break;
      await page.waitForTimeout(250);
    }
    if(!result.recordingCreated) throw new Error("Manual recording did not produce a completed file within 30 seconds.");
  }
  await page.locator('[data-record-mode="off"]').click();
  await page.waitForFunction(() => document.querySelector('[data-record-mode="off"]').getAttribute("aria-pressed")==="true");
  await page.locator(".schedule-details summary").click();
  const localInput = date => new Date(date.getTime()-date.getTimezoneOffset()*60000).toISOString().slice(0,16);
  const start = new Date(Date.now()+3600000), end = new Date(Date.now()+7200000);
  await page.locator("#schedule-start").fill(localInput(start));
  await page.locator("#schedule-end").fill(localInput(end));
  await page.locator("#add-schedule").click();
  await page.locator("#schedule-list button").last().waitFor();
  assert.equal(await page.locator('[data-record-mode="auto"]').getAttribute("aria-pressed"),"true");
  await page.locator("#schedule-list button").last().click();
  await page.waitForFunction(() => !document.querySelector("#add-schedule").disabled);
  await page.locator("#archive-camera").selectOption(stream);
  await page.locator("#archive-from").fill(localInput(new Date(Date.now()-86400000)));
  await page.locator("#refresh-recordings").click();
  await page.waitForFunction(() => !document.querySelector("#refresh-recordings").disabled);
  if (!live) {
    assert.equal(await page.locator("#recordings article").count(),1);
    assert.match(await page.locator("#recordings a").getAttribute("href"),/download=1/);
    await page.locator('[data-rewind="30"]').click();
    await page.waitForFunction(() => document.querySelector("#error").textContent.includes("아직 되감기"));
    assert.equal(changes.length,4);
    assert.equal(changes[2].schedules[0].repeat_seconds,null);
    assert.equal(changes[2].schedules[0].end_ms-changes[2].schedules[0].start_ms,3600000);
  } else {
    // Stopping immediately after a full segment can also publish a valid,
    // shorter final tail. Play the full segment we explicitly waited for.
    const recordingRow = page.locator("#recordings article").filter({has:page.locator('a[href*="'+result.recordingCreated.file+'"]')});
    await recordingRow.locator("button").click();
    await page.waitForFunction(()=>document.querySelector("#video").videoWidth>0 && document.querySelector("#video").currentTime>0.2,null,{timeout:30000});
    result.recordingPlayback=true;
    const download = await recordingRow.locator("a").getAttribute("href");
    const downloaded = await page.request.get(download,{headers:{Range:"bytes=0-63"}});
    if(downloaded.status()!==206 || !downloaded.headers()["content-disposition"]?.includes("attachment")) throw new Error("Recording range download/attachment failed");
    result.recordingDownload=true;
    await page.locator("#disconnect").click();
    await page.locator("#stream").selectOption(stream);
    await page.locator("#mode").selectOption("hls");
    await page.locator("#connect").click();
    await page.waitForFunction(() => document.querySelector("#video").videoWidth > 0 && Number(document.querySelector("#frames").textContent.replaceAll(",",""))>30,null,{timeout:60000});
    result.hls=true;
    await page.locator('[data-rewind="30"]').click();
    await page.waitForFunction(() => document.querySelector("#freshness").textContent==="되감기",null,{timeout:30000});
    result.dvr=true;
    await page.locator("#return-live").click();
    await page.waitForFunction(() => document.querySelector("#freshness").textContent==="실시간",null,{timeout:30000});
  }
  if (live && !await page.locator("#sound").isDisabled()) { await page.locator("#sound").click(); assert.equal(await page.locator("#video").evaluate(v=>v.muted),false); }
  else assert.equal(await page.locator("#sound").isDisabled(),true,"no active audio must not imply sound availability");
  await page.locator(".side-panel").evaluate(e => {e.scrollTop=0;});
  await page.screenshot({path:path.join(output,"desktop.png"),fullPage:true,mask:[page.locator("video")],maskColor:"#111719"});
  for (const width of [930,390]) {
    await page.setViewportSize({width,height:844});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1),false,"horizontal overflow at "+width);
  }
  await page.screenshot({path:path.join(output,"mobile.png"),fullPage:true,mask:[page.locator("video")],maskColor:"#111719"});
  assert.deepEqual(errors,[]); result.passed=true;
} catch(error) {result.error=error.message; await page.screenshot({path:path.join(output,"failure.png"),fullPage:true,mask:[page.locator("video")]}).catch(()=>{});}
finally {
  if(original) await page.request.post(base+"/api/recording/"+encodeURIComponent(stream),{data:{mode:original.mode,schedules:original.schedules}}).then(response=>{if(!response.ok()) throw new Error("restore HTTP "+response.status());}).catch(error=>{result.restoreError=error.message;result.passed=false;});
  await browser.close(); if(server) await new Promise(resolve=>server.close(resolve));
  await writeFile(path.join(output,"result.json"),JSON.stringify(result,null,2));
}
console.log(JSON.stringify(result,null,2)); if(!result.passed) process.exitCode=1;
