import { browserPath } from "./browser-path.mjs";
// Bounded wire-level regression for the original MSE command vocabulary.
import { chromium } from 'playwright';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
const base = new URL(process.env.UVMS_TEST_URL || 'http://127.0.0.1:8088/');
const output = path.resolve(process.env.UVMS_COMMAND_EVIDENCE || 'evidence/runtime/mse-commands');
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ executablePath: browserPath, headless: true });
const context = await browser.newContext();
const result = { passed: false, server: base.origin };
try {

  const page = await context.newPage();
  await page.goto(new URL('/healthz', base).href);
  result.protocol = await page.evaluate(async ({ alias, nextAlias }) => {
    const ws = new WebSocket(`${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/${alias}`);
    ws.binaryType = 'arraybuffer';
    const packets = []; let failure, notify;
    ws.onmessage = ({ data }) => { packets.push(new Uint8Array(data)); notify?.(); };
    ws.onerror = () => { failure = 'WebSocket error'; notify?.(); };
    ws.onclose = () => { failure = 'WebSocket closed'; notify?.(); };
    function box(view, start, end, type) {
      for (let p = start; p + 8 <= end;) {
        const size = view.getUint32(p); if (size < 8 || p + size > end) throw new Error('Malformed MP4 box');
        const name = String.fromCharCode(...new Uint8Array(view.buffer, p + 4, 4));
        if (name === type) return p;
        if (name === 'moof' || name === 'traf') { const child = box(view, p + 8, p + size, type); if (child !== undefined) return child; }
        p += size;
      }
    }
    function media(bytes) {
      const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      const tfdt = box(view, 10, bytes.length, 'tfdt'), trun = box(view, 10, bytes.length, 'trun');
      if (tfdt === undefined || trun === undefined) throw new Error('Missing fragment timestamp/sample run');
      const timestamp90k = view.getUint8(tfdt + 8) === 1 ? Number(view.getBigUint64(tfdt + 12)) : view.getUint32(tfdt + 12);
      return { timestamp90k, timestampMs: view.getInt32(6, true), duration90k: view.getUint32(trun + 20), keyframe: view.getUint32(trun + 28) === 0x02000000 };
    }
    async function next(timeout = 8000) {
      const end = Date.now() + timeout;
      while (!packets.length) {
        if (failure) throw new Error(failure);
        const wait = end - Date.now(); if (wait <= 0) throw new Error('Packet deadline');
        await new Promise(resolve => { const timer = setTimeout(() => { notify = undefined; resolve(); }, wait); notify = () => { clearTimeout(timer); notify = undefined; resolve(); }; });
      }
      const value = packets.shift();
      if (value[0] === 0) throw new Error(new TextDecoder('utf-16le').decode(value.subarray(2)));
      return value;
    }
    let last, count = 0, initializations = 0;
    function record(bytes) {
      if (bytes[0] !== 2) return undefined;
      const sample = media(bytes);
      if (sample.timestampMs !== Math.floor(sample.timestamp90k / 90)) throw new Error('Envelope and tfdt timestamp disagree');
      if (last && sample.timestamp90k < last.timestamp90k + last.duration90k) throw new Error('Timeline moved backward or samples overlap');
      last = sample; count++; return sample;
    }
    async function start() {
      let bytes;
      do { bytes = await next(); record(bytes); } while (bytes[0] !== 1);
      initializations++;
      const previous = last;
      do { bytes = await next(); } while (bytes[0] !== 2);
      const first = record(bytes);
      if (!first.keyframe) throw new Error('First fragment after init is not IDR');
      if (previous && first.timestamp90k !== previous.timestamp90k + previous.duration90k) throw new Error('Resume/switch did not join the existing playback timeline');
      return { previous, first };
    }
    try {
      const initial = await start();
      while (count < 20) record(await next());
      ws.send('pause');
      // Permit already-in-flight TCP data, then verify a full quiet interval.
      await new Promise(resolve => setTimeout(resolve, 300));
      while (packets.length) record(await next());
      const pausedAt = last;
      await new Promise(resolve => setTimeout(resolve, 1000));
      if (packets.some(p => p[0] === 2)) throw new Error('Media continued while paused');
      ws.send('continue');
      const resumed = await start();
      for (let n = 0; n < 15;) { if (record(await next())) n++; }
      ws.send(`switch:${nextAlias}`);
      const switched = await start();
      for (let n = 0; n < 15;) { if (record(await next())) n++; }
      return { passed: true, initial, pausedAt, pausedQuietMs: 1000, resumed, switched, final: last, mediaCount: count, initializations };
    } finally { ws.close(); }
  }, { alias: process.env.UVMS_TEST_STREAM || 'camera', nextAlias: process.env.UVMS_SWITCH_STREAM || 'camera_copy' });
  result.passed = result.protocol.passed;
} catch (error) { result.error = error.message; }
finally { await browser.close(); await writeFile(path.join(output, 'result.json'), JSON.stringify(result, null, 2)); console.log(JSON.stringify(result, null, 2)); }
if (!result.passed) process.exitCode = 1;
