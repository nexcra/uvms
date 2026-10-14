// Runs the extracted SDK byte-for-byte against the target service. SDK files stay
// in evidence; no third-party SDK code is copied into the service distribution.
import { chromium } from 'playwright';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import path from 'node:path';

const base = new URL(process.env.UVMS_TEST_URL || 'http://127.0.0.1:8081/');
const alias = process.env.UVMS_TEST_STREAM || 'camera';
const output = path.resolve(process.env.UVMS_SDK_EVIDENCE || 'evidence/runtime/rust-sdk');
const sdk = path.resolve('evidence/protocol/sdk/Unreal Streaming Technologies SDK v15.0');
const sources = {
  webrtc: path.join(sdk, 'WebRTC/UnrealWebRTCPublishingDemo ASP.NET Web App/UnrealWebRTCPublishingDemo/unrealwebrtcplayer.js'),
  mse: path.join(sdk, 'Players/HTML5 MSE Player/unreal_html5_player_script_v2.js'),
};
await mkdir(output, { recursive: true });
const result = { passed: false, server: base.origin, alias, authority: 'Unchanged extracted Unreal SDK players against target service', sources: {}, tests: [] };
const code = {};
for (const [kind, file] of Object.entries(sources)) {
  const bytes = await readFile(file);
  code[kind] = bytes.toString('utf8');
  result.sources[kind] = { path: file, sha256: createHash('sha256').update(bytes).digest('hex') };
}
const browser = await chromium.launch({
  executablePath: process.env.UVMS_BROWSER || 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  headless: true,
  args: ['--autoplay-policy=no-user-gesture-required', '--disable-features=WebRtcHideLocalIpsWithMdns'],
});
const context = await browser.newContext({ viewport: { width: 1040, height: 680 } });
await context.addInitScript(() => {
  window.oraclePeers = [];
  const NativePeer = window.RTCPeerConnection;
  window.RTCPeerConnection = new Proxy(NativePeer, {
    construct(target, args) {
      const peer = Reflect.construct(target, args);
      window.oraclePeers.push(peer);
      return peer;
    },
  });
});
try {

  for (const kind of ['webrtc', 'mse']) {
    const test = { kind, passed: false, errors: [], alerts: [], sockets: [], console: [] };
    result.tests.push(test);
    const page = await context.newPage();
    page.on('pageerror', error => test.errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') test.console.push(message.text()); });
    page.on('dialog', async dialog => { test.alerts.push(dialog.message()); await dialog.dismiss(); });
    page.on('websocket', ws => {
      const record = { path: new URL(ws.url()).pathname, openedAt: Date.now(), sent: [], received: [], binaryMessages: 0, binaryBytes: 0 };
      test.sockets.push(record);
      ws.on('framesent', ({ payload }) => {
        if (typeof payload !== 'string') return;
        if (payload.startsWith('{')) {
          try { const obj = JSON.parse(payload); record.sent.push({ type: obj.type, hasCandidates: /a=candidate:/.test(obj.sdp || ''), at: Date.now() }); } catch { record.sent.push({ invalidJson: true }); }
        } else record.sent.push({ command: payload.slice(0, 80), at: Date.now() });
      });
      ws.on('framereceived', ({ payload }) => {
        if (typeof payload !== 'string') { record.binaryMessages++; record.binaryBytes += payload.length; return; }
        record.received.push({ codec: payload.startsWith('H264|-|-|'), delimiterParts: payload.split('|-|-|').length, error: payload.startsWith('Error:') ? payload : undefined, at: Date.now() });
      });
      ws.on('close', () => { record.closedAt = Date.now(); });
      ws.on('socketerror', error => test.errors.push(String(error)));
    });
    try {
      // A real loopback response establishes the local address space. A mocked
      // route.fulfill navigation is classified public by Chromium's LNA checks.
      await page.goto(new URL('/healthz', base).href);
      await page.setContent('<!doctype html><html><body style="background:#111;color:#eee"><h2>Original SDK / target service</h2><video id="rtcVideo" autoplay muted playsinline width="960" height="540"></video><div id="msePlayer"></div></body></html>');
      await page.addScriptTag({ content: code[kind] });
      await page.evaluate(({ kind, alias, host, port, secure }) => {
        if (kind === 'webrtc') {
          window.sdkPlayer = new UnrealWebRTCPlayer('rtcVideo', alias, '', host, port, secure, false, 'udp');
          window.sdkPlayer.Play();
        } else {
          document.getElementById('rtcVideo').remove();
          RunPlayer('msePlayer', 960, 540, host, port, secure, alias, '', false, true, 1, '', false);
          document.getElementById('msePlayer_Video').muted = true;
          document.getElementById('msePlayer_playpause').click();
        }
      }, { kind, alias, host: base.hostname, port: Number(base.port || (base.protocol === 'https:' ? 443 : 80)), secure: base.protocol === 'https:' });
      const id = kind === 'webrtc' ? 'rtcVideo' : 'msePlayer_Video';
      await page.waitForFunction(id => {
        const v = document.getElementById(id);
        return v?.videoWidth > 0 && v.getVideoPlaybackQuality().totalVideoFrames >= 35;
      }, id, { timeout: 30000 });
      test.first = await videoState(page, id);
      if (kind === 'webrtc') {
        const signal = test.sockets.find(s => s.path.startsWith('/webrtc_playnow/'));
        if (!signal?.closedAt) throw new Error('SDK signaling socket did not close');
        if (!signal.sent.some(s => s.type === 'offer' && !s.hasCandidates)) throw new Error('Original SDK candidate-free offer was not observed');
      }
      await page.waitForTimeout(5200);
      test.after = await videoState(page, id);
      if (test.after.frames - test.first.frames < 20 || test.after.time <= test.first.time + 3) throw new Error('Playback did not continue for five seconds');
      if (test.errors.length || test.alerts.length) throw new Error('SDK reported a browser error or alert');
      test.passed = true;
      await page.screenshot({ path: path.join(output, `${kind}.png`), mask: [page.locator('video')], maskColor: '#111719' });
    } catch (error) { test.error = error.message; }
    finally {
      test.stats = await page.evaluate(async () => {
        const out = [];
        for (const peer of window.oraclePeers || []) {
          out.push({ connectionState: peer.connectionState, iceConnectionState: peer.iceConnectionState, reports: [...(await peer.getStats()).values()].filter(s => ['inbound-rtp', 'transport', 'codec', 'candidate-pair'].includes(s.type)) });
        }
        return out;
      }).catch(() => []);
      await page.evaluate(() => { window.sdkPlayer?.Stop(); document.getElementById('msePlayer_stop')?.click(); }).catch(() => {});
      await page.close();
      await writeFile(path.join(output, `${kind}.json`), JSON.stringify(test, null, 2));
      console.log(JSON.stringify({ kind, passed: test.passed, error: test.error, alerts: test.alerts }));
    }
  }
  result.passed = result.tests.every(test => test.passed);
} catch (error) { result.error = error.message; }
finally {
  await browser.close();
  await writeFile(path.join(output, 'browser.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify({ passed: result.passed, error: result.error, tests: result.tests.map(({ kind, passed, error, first, after }) => ({ kind, passed, error, first, after })) }, null, 2));
}
if (!result.passed) process.exitCode = 1;

async function videoState(page, id) {
  return page.evaluate(id => {
    const v = document.getElementById(id), q = v.getVideoPlaybackQuality();
    return { width: v.videoWidth, height: v.videoHeight, time: v.currentTime, frames: q.totalVideoFrames, dropped: q.droppedVideoFrames, at: Date.now() };
  }, id);
}
