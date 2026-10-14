import { chromium } from "playwright";
import { existsSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

// Run only against a synthetic tone fixture. This checks decoded samples, not
// merely SDP tracks or transport packet counters, and does not save audio.
const base = process.env.UVMS_TEST_URL;
const streams = (process.env.UVMS_TEST_STREAMS || "").split(",").filter(Boolean);
if (!base || !streams.length) throw new Error("Set UVMS_TEST_URL and UVMS_TEST_STREAMS to an isolated synthetic audio fixture.");
const modes = (process.env.UVMS_TEST_MODES || "webrtc,mse,hls").split(",");
const output = path.resolve(process.env.UVMS_TEST_OUTPUT || "test-results/browser-audio");
await mkdir(output,{recursive:true});
const edge = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const browser = await chromium.launch({headless:true,executablePath:process.env.UVMS_BROWSER_PATH || (existsSync(edge)?edge:undefined),args:["--autoplay-policy=no-user-gesture-required"]});
const results = [];
try {
  for (const stream of streams) for (const mode of modes) {
    const page = await browser.newPage();
    await page.addInitScript(() => {
      window.__uvmsMediaErrors = [];
      document.addEventListener("error", event => {
        if (event.target instanceof HTMLMediaElement) window.__uvmsMediaErrors.push({code:event.target.error?.code,message:event.target.error?.message,source:new URL(event.target.currentSrc || location.href).pathname});
      },true);
    });
    if (process.env.UVMS_COLD_HLS === "1") await page.route("**/app.js",async route => {
      const response = await route.fetch(), source = await response.text();
      const automatic = "loadStreams().then(ready => { if (ready && ui.stream.value) connect(); });";
      if(!source.includes(automatic)) throw new Error("Cold fixture cannot disable automatic default camera connection.");
      await route.fulfill({response,body:source.replace(automatic,"loadStreams();")});
    });
    if (process.env.UVMS_FORCE_HLS_JS === "1") await page.addInitScript(() => {
      const original = HTMLMediaElement.prototype.canPlayType;
      HTMLMediaElement.prototype.canPlayType = function(type) { return /mpegurl/i.test(type) ? "" : original.call(this,type); };
    });
    const errors = [], result = {stream,mode,hlsJsForced:process.env.UVMS_FORCE_HLS_JS === "1",passed:false,errors};
    result.httpErrors=[];
    result.hlsResponses=[];
    page.on("response",async response=>{
      const pathname = new URL(response.url()).pathname;
      if(response.status()>=400) result.httpErrors.push({path:pathname,status:response.status()});
      if(pathname.startsWith("/hls/") && result.hlsResponses.length<20) {
        const entry={path:pathname,status:response.status()}; result.hlsResponses.push(entry);
        if(pathname.endsWith(".m3u8") && result.hlsResponses.filter(r=>r.path.endsWith(".m3u8")).length<=3) entry.playlist=await response.text().catch(()=>"unavailable");
      }
    });
    page.on("pageerror",error=>errors.push(error.message));
    try {
      await page.goto(base);
      await page.waitForFunction(()=>document.querySelector("#stream").options.length && !document.querySelector("#stream").disabled);
      await page.locator("#disconnect").evaluate(b=>{if(!b.disabled)b.click();});
      await page.locator("#stream").selectOption(stream);
      await page.locator("#mode").selectOption(mode);
      await page.locator("#connect").click();
      await page.waitForFunction(()=>!document.querySelector("#error").hidden || (document.querySelector("#video").videoWidth>0 && Number(document.querySelector("#frames").textContent.replaceAll(",",""))>15),null,{timeout:60000});
      if(!await page.locator("#error").isHidden()) throw new Error(await page.locator("#error").textContent());
      result.audioLabel = await page.locator("#audio-status").textContent();
      await page.waitForFunction(()=>!document.querySelector("#sound").disabled,null,{timeout:10000});
      await page.locator("#sound").click();
      result.analysis = await page.evaluate(async () => {
        const video = document.querySelector("#video");
        const context = new AudioContext();
        await context.resume();
        const source = video.srcObject ? context.createMediaStreamSource(new MediaStream(video.srcObject.getAudioTracks())) : context.createMediaElementSource(video);
        const analyser = context.createAnalyser(); analyser.fftSize = 2048;
        source.connect(analyser); analyser.connect(context.destination);
        const values = new Float32Array(analyser.fftSize);
        let maxRms = 0, nonzeroWindows = 0;
        for(let i=0;i<40;i++) {
          await new Promise(resolve=>setTimeout(resolve,100));
          analyser.getFloatTimeDomainData(values);
          const rms = Math.sqrt(values.reduce((sum,v)=>sum+v*v,0)/values.length);
          maxRms = Math.max(maxRms,rms); if(rms>0.0001) nonzeroWindows++;
        }
        source.disconnect(); analyser.disconnect(); await context.close();
        return {maxRms,nonzeroWindows,sampleRate:context.sampleRate,muted:video.muted,videoTime:video.currentTime,audioTracks:video.srcObject?.getAudioTracks().length ?? null};
      });
      const metadata = await (await page.request.get(base+"/api/streams")).json();
      result.capability = metadata.streams.find(s=>s.id===stream)?.audio;
      result.passed = result.analysis.nonzeroWindows >= 20 && result.analysis.maxRms>0.0001 && !result.analysis.muted && errors.length===0;
      if(!result.passed) result.error="Expected decoded non-silent audio for at least 20 analyser windows.";
    } catch(error) {result.error=error.message; result.uiError=await page.locator("#error").textContent().catch(()=>"");}
    finally {result.mediaErrors=await page.evaluate(()=>window.__uvmsMediaErrors).catch(()=>[]); await page.close(); results.push(result); console.log(JSON.stringify(result));}
  }
} finally {
  await browser.close(); await writeFile(path.join(output,"audio.json"),JSON.stringify(results,null,2));
}
if(results.some(result=>!result.passed)) process.exitCode=1;
