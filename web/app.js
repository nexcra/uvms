"use strict";

const byId = (id) => document.getElementById(id);
const ui = Object.fromEntries(["stream", "mode", "transport-caption", "transport-path", "load-streams", "connect", "disconnect", "reconnect", "video", "empty", "empty-title", "empty-message", "error", "status", "status-label", "current-stream", "frames", "bytes", "fps", "resolution", "freshness", "stream-hint"].map((id) => [id, byId(id)]));
let session = null;
let listRequest = null;
let streamsLoaded = false;
let wallLayout = 1;
let wallActive = false;
let cameras = [];
const wallPlayers = new Map();
let cameraPoll;

function setStatus(state, label) {
  ui.status.dataset.state = state;
  ui["status-label"].textContent = label;
}

function showError(message) {
  ui.error.textContent = message;
  ui.error.hidden = !message;
}

function updateButtons() {
  const ready = (wallLayout > 1 ? selectedCameras().length > 0 : Boolean(ui.stream.value)) && streamsLoaded;
  ui.connect.disabled = !ready || Boolean(session) || wallActive;
  ui.reconnect.disabled = !ready;
  ui.disconnect.disabled = !session && !wallActive;
  ui["load-streams"].disabled = Boolean(listRequest);
}

function empty(title, message) {
  ui.empty.hidden = false;
  ui["empty-title"].textContent = title;
  ui["empty-message"].textContent = message;
}

function endSession(message = "연결을 해제했습니다.") {
  stopWall();
  const old = session;
  session = null;
  if (old) {
    old.abort.abort();
    clearTimeout(old.deadline);
    clearInterval(old.statsTimer);
    clearInterval(old.pingTimer);
    old.hls?.destroy();
    if (old.pc) {
      old.pc.ontrack = null;
      old.pc.onconnectionstatechange = null;
      old.pc.close();
    }
    if (old.sourceBuffer && old.mediaSource.readyState === "open") {
      try { old.sourceBuffer.abort(); old.mediaSource.removeSourceBuffer(old.sourceBuffer); } catch { /* Already detached. */ }
    }
    if (old.queue) { old.queue.length = 0; old.queueBytes = 0; }
    if (old.ws) {
      old.ws.onmessage = null;
      old.ws.onclose = null;
      old.ws.onerror = null;
      
      old.ws.close();
    }
  }
  ui.video.srcObject = null;
  ui.video.muted = true;
  updateSound();
  ui.video.removeAttribute("src");
  ui.video.load();
  if (old?.objectUrl) URL.revokeObjectURL(old.objectUrl);
  ui["current-stream"].textContent = "NO STREAM";
  ui.freshness.textContent = "대기";
  setAudioStatus("음성 수신 여부 확인 대기", false);
  setStatus("idle", "연결 대기");
  empty("영상 연결 대기", message);
  updateButtons();
}

function fail(current, message) {
  if (session !== current) return;
  endSession("설정을 확인하고 다시 연결하세요.");
  setStatus("error", "연결 오류");
  ui.freshness.textContent = "연결 끊김";
  showError(message);
}

async function loadStreams() {
  listRequest?.abort();
  const request = new AbortController();
  listRequest = request;
  showError("");
  ui["load-streams"].textContent = "목록 불러오는 중…";
  updateButtons();
  const timeout = setTimeout(() => request.abort(), 10000);
  try {
    const response = await fetch("/api/streams", { signal: request.signal, cache: "no-store" });
    if (!response.ok) throw new Error(`카메라 목록을 불러오지 못했습니다. (HTTP ${response.status})`);
    const data = await response.json();
    if (listRequest !== request) return;
    if (!Array.isArray(data.streams)) throw new Error("서버의 카메라 목록 형식이 올바르지 않습니다.");
    const previous = ui.stream.value;
    ui.stream.replaceChildren();
    for (const stream of data.streams) {
      if (typeof stream.id === "string" && stream.id) ui.stream.add(new Option(stream.id, stream.id));
    }
    streamsLoaded = true;
    cameras = data.streams;
    renderCameraChoices();
    scheduleCameraPoll();
    if ([...ui.stream.options].some((option) => option.value === previous)) ui.stream.value = previous;
    else if (data.default_stream && [...ui.stream.options].some(option => option.value === data.default_stream)) ui.stream.value = data.default_stream;
    ui.stream.disabled = ui.stream.options.length === 0;
    if (!ui.stream.options.length) {
      ui.stream.add(new Option("등록된 카메라가 없습니다", ""));
      ui["stream-hint"].textContent = "서버 설정에 RTSP 카메라를 등록하세요.";
    } else {
      ui["stream-hint"].textContent = `${ui.stream.options.length}개 카메라 · 재생할 카메라를 선택하세요.`;
    }
    populateSurveillanceCameras();
    return true;
  } catch (error) {
    if (listRequest === request) showError(error.name === "AbortError" ? "목록 요청 시간이 초과됐습니다. 서버 연결을 확인하세요." : error.message);
  } finally {
    clearTimeout(timeout);
    if (listRequest === request) {
      listRequest = null;
      ui["load-streams"].textContent = "카메라 목록 새로고침";
      updateButtons();
    }
  }
}

function gatherIce(pc, signal) {
  if (pc.iceGatheringState === "complete") return Promise.resolve();
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      clearTimeout(timeout);
      pc.removeEventListener("icegatheringstatechange", changed);
      signal.removeEventListener("abort", cancelled);
    };
    const changed = () => {
      if (pc.iceGatheringState === "complete") { cleanup(); resolve(); }
    };
    const cancelled = () => { cleanup(); reject(new DOMException("연결 취소", "AbortError")); };
    const timeout = setTimeout(() => { cleanup(); reject(new Error("네트워크 경로 확인 시간이 초과됐습니다.")); }, 10000);
    pc.addEventListener("icegatheringstatechange", changed);
    signal.addEventListener("abort", cancelled, { once: true });
    if (signal.aborted) cancelled();
    else changed();
  });
}

function openSocket(ws, signal) {
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      clearTimeout(timeout);
      ws.removeEventListener("open", opened);
      ws.removeEventListener("error", errored);
      ws.removeEventListener("close", errored);
      signal.removeEventListener("abort", cancelled);
    };
    const opened = () => { cleanup(); resolve(); };
    const errored = () => { cleanup(); reject(new Error("영상 서버에 연결하지 못했습니다.")); };
    const cancelled = () => { cleanup(); reject(new DOMException("연결 취소", "AbortError")); };
    const timeout = setTimeout(() => { cleanup(); reject(new Error("영상 서버 응답 시간이 초과됐습니다.")); }, 10000);
    ws.addEventListener("open", opened, { once: true });
    ws.addEventListener("error", errored, { once: true });
    ws.addEventListener("close", errored, { once: true });
    signal.addEventListener("abort", cancelled, { once: true });
    if (signal.aborted) cancelled();
  });
}

async function receiveStats(current) {
  if (current.readingStats || session !== current) return;
  current.readingStats = true;
  try {
    let frames = 0, bytes = 0, width = 0, height = 0;
    if (current.mode !== "webrtc") {
      const quality = ui.video.getVideoPlaybackQuality?.();
      frames = quality ? Math.max(0, quality.totalVideoFrames - quality.droppedVideoFrames) : (ui.video.webkitDecodedFrameCount || 0);
      bytes = current.receivedBytes || 0;
      width = ui.video.videoWidth;
      height = ui.video.videoHeight;
    } else {
      const report = await current.pc.getStats();
      if (session !== current) return;
      for (const item of report.values()) {
        if (item.type === "inbound-rtp" && (item.kind === "video" || item.mediaType === "video")) {
          frames += item.framesDecoded || 0;
          bytes += item.bytesReceived || 0;
          width = item.frameWidth || width;
          height = item.frameHeight || height;
        }
      }
    }
    const now = performance.now();
    const frameDelta = Math.max(0, frames - current.frames);
    const fps = current.lastStatsAt ? frameDelta * 1000 / (now - current.lastStatsAt) : 0;
    if (frameDelta > 0) current.lastFrameAt = now;
    current.frames = frames;
    current.lastStatsAt = now;
    ui.frames.textContent = frames.toLocaleString("ko-KR");
    ui.bytes.textContent = current.mode === "file" || current.mode === "hls" ? "—" : (bytes / 1000000).toFixed(2);
    ui.fps.textContent = fps.toFixed(1);
    ui.resolution.textContent = width && height ? `${width}×${height}` : "—";
    if (current.mode !== "webrtc" || current.pc.connectionState === "connected") {
      if (frames > 0 && now - current.lastFrameAt < 5000) {
        if (current.mode !== "webrtc") clearTimeout(current.deadline);
        ui.empty.hidden = true;
        setStatus("live", current.mode === "file" ? "녹화 재생 중" : current.from ? "되감기 재생 중" : "영상 수신 중");
        ui.freshness.textContent = current.mode === "file" ? "녹화" : current.from ? "되감기" : "실시간";
      } else if ((current.mode === "file" || current.from) && (ui.video.paused || ui.video.ended)) {
        setStatus("idle", ui.video.ended ? "재생 완료" : "일시 정지");
        ui.freshness.textContent = ui.video.ended ? "완료" : "일시 정지";
      } else if (now - current.startedAt > 10000) {
        setStatus("stale", "영상 수신 대기");
        ui.freshness.textContent = frames ? "수신 지연" : "프레임 대기";
        if (!frames) empty("영상 신호를 기다리는 중", "카메라 연결과 H.264 인코딩 설정을 확인하세요.");
      }
    }
  } catch {
    // The connection can close while a stats request is pending.
  } finally {
    current.readingStats = false;
  }
}

function playVideo(current) {
  ui.video.play().catch((error) => {
    if (error.name === "AbortError") return;
    if (session === current) { ui.empty.hidden = true; showError("영상의 재생 버튼을 눌러 재생을 시작하세요."); }
  });
}

function syncLiveEdge(current) {
  if (session !== current || !ui.video.buffered.length) return;
  const ranges = ui.video.buffered;
  const start = ranges.start(ranges.length - 1);
  const end = ranges.end(ranges.length - 1);
  if (ui.video.currentTime < start || end - ui.video.currentTime > 2) ui.video.currentTime = Math.max(start, end - 0.3);
}

function pumpMse(current) {
  if (session !== current || current.mediaSource.readyState !== "open") return;
  try {
    if (!current.sourceBuffer) {
      if (!current.mime) return;
      current.sourceBuffer = current.mediaSource.addSourceBuffer(current.mime);
      current.sourceBuffer.addEventListener("updateend", () => {
        syncLiveEdge(current);
        pumpMse(current);
      }, { signal: current.abort.signal });
      current.sourceBuffer.addEventListener("error", () => fail(current, "영상 조각을 해석하지 못했습니다. 카메라 인코딩을 확인하거나 WebRTC로 연결하세요."), { signal: current.abort.signal });
    }
    const buffer = current.sourceBuffer;
    if (buffer.updating) return;
    const pruneBefore = ui.video.currentTime - 10;
    if (buffer.buffered.length && pruneBefore > 0 && pruneBefore - current.lastPrune > 3 && buffer.buffered.start(0) < pruneBefore) {
      current.lastPrune = pruneBefore;
      buffer.remove(0, pruneBefore);
      return;
    }
    if (current.queue.length) {
      const data = current.queue.shift();
      current.queueBytes -= data.byteLength;
      buffer.appendBuffer(data);
    }
  } catch (error) {
    fail(current, error.name === "QuotaExceededError" ? "브라우저 영상 버퍼가 가득 찼습니다. 다시 연결하거나 더 낮은 해상도를 사용하세요." : "WebSocket 영상을 재생하지 못했습니다. 카메라의 H.264 설정을 확인하세요.");
  }
}

async function connectMse(current) {
  try {
    current.queue = [];
    current.queueBytes = 0;
    current.receivedBytes = 0;
    current.lastPrune = 0;
    current.mediaSource = new MediaSource();
    current.mediaSource.addEventListener("sourceopen", () => pumpMse(current), { once: true, signal: current.abort.signal });
    current.objectUrl = URL.createObjectURL(current.mediaSource);
    ui.video.addEventListener("canplay", () => { syncLiveEdge(current); playVideo(current); }, { once: true, signal: current.abort.signal });
    ui.video.addEventListener("error", () => fail(current, "브라우저가 영상을 디코딩하지 못했습니다. 카메라의 H.264 설정을 확인하세요."), { signal: current.abort.signal });
    ui.video.src = current.objectUrl;
    const wsUrl = new URL("/" + encodeURIComponent(current.stream), location.href);
    wsUrl.protocol = location.protocol === "https:" ? "wss:" : "ws:";
    const ws = new WebSocket(wsUrl);
    current.ws = ws;
    ws.binaryType = "arraybuffer";
    ws.onmessage = (event) => {
      if (session !== current) return;
      try {
        if (!(event.data instanceof ArrayBuffer)) throw new Error("영상 메시지 형식 오류");
        const view = new DataView(event.data);
        const type = view.getUint8(0);
        let data;
        if (type === 0) throw new Error(new TextDecoder("utf-16le").decode(event.data.slice(2)));
        if (type === 1) {
          if (view.byteLength < 20) throw new Error("초기화 메시지 오류");
          const length = view.getUint16(18, true);
          const xml = new DOMParser().parseFromString(new TextDecoder("utf-16le").decode(event.data.slice(20, 20 + length)), "text/xml");
          current.mime = xml.querySelector("mimetypecodec")?.textContent;
          const hasAudio = /mp4a|opus|alaw|ulaw/i.test(current.mime || "");
          setAudioStatus(hasAudio ? "음성 트랙 포함 · 소리를 켜서 들으세요." : "현재 MSE 스트림은 영상만 수신합니다.", hasAudio);
          if (!current.mime || !MediaSource.isTypeSupported(current.mime)) throw new Error("지원하지 않는 H.264 형식입니다.");
          data = event.data.slice(20 + length);
        } else if (type === 2) {
          if (!current.mime || view.byteLength < 10) throw new Error("영상 초기화가 필요합니다.");
          data = event.data.slice(10);
        } else throw new Error("알 수 없는 영상 메시지입니다.");
        if (current.queueBytes + data.byteLength > 32 * 1024 * 1024 || current.queue.length >= 256) throw new Error("영상 버퍼가 가득 찼습니다. 다시 연결하세요.");
        current.receivedBytes += data.byteLength;
        current.queueBytes += data.byteLength;
        current.queue.push(data);
        pumpMse(current);
      } catch (error) { fail(current, error.message); }
    };
    ws.onclose = () => fail(current, "서버 연결이 종료됐습니다. 다시 연결하세요.");
    ws.onerror = () => fail(current, "서버 연결에 오류가 발생했습니다.");
    await openSocket(ws, current.abort.signal);
    if (session !== current) return;

    current.statsTimer = setInterval(() => receiveStats(current), 1000);

  } catch (error) {
    if (error.name !== "AbortError") fail(current, error.message || "WebSocket 영상 연결을 시작하지 못했습니다.");
  }
}

async function connect(options = {}) {
  if (wallLayout > 1) { startWall(); return; }
  if (!ui.stream.value || !streamsLoaded) return;
  endSession();
  showError("");
  const mode = ui.mode.value;
  if (mode === "webrtc" && !window.RTCPeerConnection) { showError("이 브라우저는 WebRTC를 지원하지 않습니다."); return; }
  if (mode === "mse" && !window.MediaSource) { showError("이 브라우저는 WebSocket 영상 재생을 지원하지 않습니다. WebRTC로 연결하세요."); return; }
  const pc = mode === "webrtc" ? new RTCPeerConnection() : null;
  const current = { pc, mode, abort: new AbortController(), stream: ui.stream.value, frames: 0, lastStatsAt: 0, lastFrameAt: 0, startedAt: performance.now() };
  session = current;
  warmDvr(current.stream);
  current.from = options.from || null;
  setAudioStatus("음성 트랙 확인 중");
  ui["current-stream"].textContent = ui.stream.value;
  ui["transport-caption"].textContent = mode === "mse" ? "WEBSOCKET" : mode.toUpperCase();
  ui["transport-path"].textContent = "RTSP → " + (mode === "mse" ? "WEBSOCKET" : mode.toUpperCase());
  for (const key of ["frames", "bytes", "fps", "resolution"]) ui[key].textContent = "—";
  setStatus("connecting", "연결 중");
  empty("카메라에 연결하는 중", "네트워크 경로와 영상 신호를 확인하고 있습니다.");
  ui.freshness.textContent = "연결 중";
  updateButtons();
  current.deadline = setTimeout(() => fail(current, "영상 연결 시간이 초과됐습니다. 서버의 미디어 포트와 카메라 상태를 확인하세요."), mode === "hls" ? 60000 : 30000);
  if (mode === "hls") { connectHls(current); return; }
  if (mode === "mse") { await connectMse(current); return; }
  pc.addTransceiver("video", { direction: "recvonly" });
  pc.addTransceiver("audio", { direction: "recvonly" });
  current.tracks = new MediaStream();
  pc.ontrack = (event) => {
    if (session !== current) return;
    current.tracks.addTrack(event.track);
    if (ui.video.srcObject !== current.tracks) ui.video.srcObject = current.tracks;
    if (event.track.kind === "audio") setAudioStatus("음성 트랙 연결됨 · 소리를 켜서 들으세요.", true);
    playVideo(current);
  };
  pc.onconnectionstatechange = () => {
    if (session !== current) return;
    if (pc.connectionState === "connected") {
      clearTimeout(current.deadline);
      setStatus("connecting", "영상 신호 확인 중");
      current.statsTimer ||= setInterval(() => receiveStats(current), 1000);
    } else if (pc.connectionState === "failed" || pc.connectionState === "closed") {
      fail(current, "영상 연결이 끊겼습니다. 다시 연결하세요.");
    } else if (pc.connectionState === "disconnected") {
      setStatus("stale", "네트워크 연결 지연");
      ui.freshness.textContent = "수신 지연";
      clearTimeout(current.deadline);
      current.deadline = setTimeout(() => fail(current, "네트워크 연결이 복구되지 않았습니다. 다시 연결하세요."), 15000);
    }
  };
  try {
    const wsUrl = new URL("/webrtc_playnow/randomport/udp/" + encodeURIComponent(current.stream), location.href);
    wsUrl.protocol = location.protocol === "https:" ? "wss:" : "ws:";
    const ws = new WebSocket(wsUrl);
    current.ws = ws;
    ws.onmessage = async (event) => {
      if (session !== current) return;
      try {
        if (typeof event.data === "string" && event.data.startsWith("H264|-|-|")) {
          const caps = RTCRtpReceiver.getCapabilities("video").codecs.filter(c => c.mimeType.toLowerCase() === "video/h264");
          pc.getTransceivers()[0].setCodecPreferences(caps);
          const offer = await pc.createOffer();
          offer.sdp = offer.sdp.replaceAll("a=extmap-allow-mixed\r\n", "");
          await pc.setLocalDescription(offer);
          await gatherIce(pc, current.abort.signal);
          if (session === current && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({type:"offer",sdp:pc.localDescription.sdp}));
        } else {
          const parts = event.data.split("|-|-|");
          const answer = JSON.parse(parts[0]);
          if (answer.type !== "answer") throw new Error(answer.message || "영상 연결 응답 오류");
          current.answerReceived = true;
          await pc.setRemoteDescription(answer);
          if (!current.tracks.getAudioTracks().length) setAudioStatus("현재 WebRTC 연결에 음성 트랙이 없습니다.", false);
          if (parts[1]) await pc.addIceCandidate(JSON.parse(parts[1]));
          current.answerReceived = true;
          ws.close();
        }
      } catch (error) { fail(current, error.message); }
    };
    ws.onclose = () => { if (!current.answerReceived) fail(current, "영상 협상이 종료됐습니다."); };
    ws.onerror = () => { if (!current.answerReceived) fail(current, "서버 연결 오류"); };
    await openSocket(ws, current.abort.signal);
  } catch (error) {
    if (error.name !== "AbortError") fail(current, error.message || "영상 연결을 시작하지 못했습니다.");
  }
}

byId("connection-form").addEventListener("submit", (event) => { event.preventDefault(); connect(); });
ui["load-streams"].addEventListener("click", loadStreams);
ui.disconnect.addEventListener("click", () => { showError(""); endSession(); });
ui.reconnect.addEventListener("click", connect);
ui.stream.addEventListener("change", updateButtons);
window.addEventListener("pagehide", () => { clearTimeout(cameraPoll); listRequest?.abort(); endSession(); streamsLoaded = false; });
updateButtons();

let archiveRequest = 0, archiveOffset = 0, archiveFilter = "";
let controlRequest = 0, recordingControl = null, controlSaving = false;
const dateLabel = value => new Date(Number(value)).toLocaleString("ko-KR");
function setAudioStatus(message, available = null) {
  byId("audio-status").textContent = message;
  byId("audio-status").dataset.state = "normal";
  if (available !== null) byId("sound").disabled = !available;
}
function audioCapability(camera, mode) {
  const audio = camera?.audio;
  if (!audio) return null;
  if (audio.state === "offline") return {text:"카메라 연결 후 음성 여부를 확인합니다.",available:false,state:"normal"};
  if (audio.state === "no_audio") return {text:"이 카메라 입력에는 음성 트랙이 없습니다.",available:false,state:"normal"};
  if (audio.state === "unsupported") return {text:"카메라 음성 형식을 지원하지 않습니다. AAC·G.711·Opus 설정을 확인해 주세요.",available:false,state:"warning"};
  const output = mode === "webrtc" ? audio.webrtc : audio.archive;
  if (audio.state === "conversion_errors") return {text:"음성 처리 오류가 발생했습니다. 카메라 음성 설정을 확인해 주세요.",available:Boolean(output),state:"warning"};
  if (!output) return {text:"이 재생 방식에서 카메라 음성을 사용할 수 없습니다.",available:false,state:"warning"};
  return {text:"음성 " + (audio.input?.codec || output.codec) + " · 소리를 켜서 들으세요.",available:true,state:"normal"};
}
function refreshAudioCapability() {
  if (!session || session.mode === "file") return;
  const info = audioCapability(cameras.find(c => c.id === session.stream), session.mode);
  if (!info) return;
  // A newly negotiated track is stronger evidence than an older offline poll.
  if (info.text.startsWith("카메라 연결 후") && session.tracks?.getAudioTracks().length) return;
  setAudioStatus(info.text, info.available); byId("audio-status").dataset.state = info.state;
}
function updateSound() {
  byId("sound").textContent = ui.video.muted ? "소리 켜기" : "소리 끄기";
  byId("sound").setAttribute("aria-pressed", String(!ui.video.muted));
}
byId("sound").addEventListener("click", () => { ui.video.muted = !ui.video.muted; updateSound(); if (session) playVideo(session); });
ui.video.addEventListener("volumechange", updateSound);

function attachHls(video, url, from, owner, onError, onAudio) {
  if (video.canPlayType("application/vnd.apple.mpegurl")) {
    video.src = url;
    video.addEventListener("loadedmetadata", () => {
      if (from && video.seekable.length) video.currentTime = video.seekable.start(0);
      const hasAudio = video.audioTracks ? video.audioTracks.length > 0 : null;
      onAudio?.(hasAudio === true ? "HLS 음성 트랙 포함 · 소리를 켜서 들으세요." : hasAudio === false ? "현재 HLS 스트림은 영상만 수신합니다." : "HLS 음성 수신 여부를 확인하고 있습니다.",hasAudio);
      video.play().catch(() => onError("영상의 재생 버튼을 눌러 주세요."));
    }, {once: true, signal: owner.abort.signal});
  } else if (window.Hls?.isSupported()) {
    const hls = owner.hls = new Hls({startPosition: from ? 0 : -1, maxBufferLength: 20, backBufferLength: 30, ...(from ? {liveSyncDurationCount: 180, liveMaxLatencyDurationCount: Infinity} : {})});
    hls.on(Hls.Events.ERROR, (_, data) => { if (data.fatal) onError("HLS 영상을 불러오지 못했습니다. 입력 신호와 저장된 영상 범위를 확인해 주세요."); });
    hls.on(Hls.Events.MANIFEST_PARSED, () => video.play().catch(() => onError("영상의 재생 버튼을 눌러 주세요.")));
    hls.on(Hls.Events.BUFFER_CODECS, (_, tracks) => {
      const hasAudio = Boolean(tracks.audio || Object.values(tracks).some(t => /mp4a|opus/i.test(t.codec || "")));
      onAudio?.(hasAudio ? "HLS 음성 트랙 포함 · 소리를 켜서 들으세요." : "현재 HLS 스트림은 영상만 수신합니다.",hasAudio);
    });
    hls.loadSource(url); hls.attachMedia(video);
  } else onError("이 브라우저는 HLS를 지원하지 않습니다. WebRTC로 연결해 주세요.");
}
function connectHls(current) {
  const url = new URL("/hls/" + encodeURIComponent(current.stream) + "/index.m3u8", location.href);
  if (current.from) url.searchParams.set("from", String(current.from));
  ui.video.addEventListener("error", () => fail(current, "HLS 디코딩 오류가 발생했습니다."), {signal: current.abort.signal});
  attachHls(ui.video, url.href, current.from, current, message => fail(current, message), setAudioStatus);
  current.statsTimer = setInterval(() => receiveStats(current), 1000);
  byId("playback-hint").textContent = current.from ? dateLabel(current.from) + " 부근부터 재생 · 영상 조각의 시작 시간으로 맞춰집니다." : "HLS 실시간 재생 · 서버에 쌓인 영상 범위에서 되감기할 수 있습니다.";
}
function warmDvr(stream) {
  // Watching live also starts the bounded rewind cache for this same shared
  // camera feed. Status failures must not interrupt WebRTC or MSE playback.
  jsonRequest("/api/hls/" + encodeURIComponent(stream)).catch(() => {});
}
async function rewind(seconds) {
  if (!ui.stream.value) return;
  const stream = ui.stream.value;
  try {
    const state = await jsonRequest("/api/hls/" + encodeURIComponent(stream));
    if (!state.ready || !state.oldest_ms || !state.newest_ms) throw new Error("아직 되감기할 영상이 없습니다. HLS로 연결한 뒤 영상이 쌓일 때까지 기다려 주세요.");
    const requested = Date.now() - seconds * 1000;
    const from = Math.max(state.oldest_ms, Math.min(requested, state.newest_ms));
    if (ui.stream.value !== stream) return;
    applyLayout(1); ui.mode.value = "hls"; await connect({from});
    if (from > requested) byId("playback-hint").textContent = "저장된 가장 이른 영상부터 재생합니다: " + dateLabel(from);
    if (state.error) showError("현재 카메라 입력이 원활하지 않습니다. 남아 있는 되감기 영상을 재생합니다.");
  } catch (error) { showError(error.message); }
}
document.querySelectorAll("[data-rewind]").forEach(button => button.addEventListener("click", () => rewind(Number(button.dataset.rewind))));
byId("return-live").addEventListener("click", () => { if (!ui.stream.value) return; byId("playback-hint").textContent = "현재 카메라의 실시간 영상으로 돌아갑니다."; connect(); });

async function jsonRequest(url, options = {}) {
  const response = await fetch(url, {cache: "no-store", signal: AbortSignal.timeout(10000), ...options});
  if (!response.ok) {
    if (response.status === 404) throw new Error("이 서버에서 해당 관제 기능을 사용할 수 없습니다.");
    throw new Error("요청을 처리하지 못했습니다. (HTTP " + response.status + ")");
  }
  return response.json();
}
function populateSurveillanceCameras() {
  for (const id of ["record-camera", "archive-camera"]) {
    const select = byId(id), previous = select.value;
    select.replaceChildren();
    if (id === "archive-camera") select.add(new Option("모든 카메라", ""));
    for (const camera of cameras) select.add(new Option(camera.id, camera.id));
    select.value = [...select.options].some(o => o.value === previous) ? previous : ui.stream.value;
  }
  refreshControl();
}
function renderControl(data) {
  recordingControl = data;
  const label = byId("recording-state");
  label.dataset.state = data.status;
  label.textContent = ({recording:"● 녹화 중", off:"녹화 꺼짐", scheduled:"예약 대기", connecting:"입력 연결 중", storage_full:"저장 한도 도달",retrying:"녹화 재시도",stopped:"녹화 중지"}[data.status] || "녹화 상태 확인 중");
  document.querySelectorAll("[data-record-mode]").forEach(button => { button.disabled = controlSaving; button.setAttribute("aria-pressed", String(button.dataset.recordMode === data.mode)); });
  const list = byId("schedule-list"); list.replaceChildren();
  (data.schedules || []).forEach((schedule, index) => {
    const row = document.createElement("div"), text = document.createElement("span"), remove = document.createElement("button");
    text.textContent = dateLabel(schedule.start_ms) + " → " + dateLabel(schedule.end_ms) + (schedule.repeat_seconds === 86400 ? " · 매일" : schedule.repeat_seconds === 604800 ? " · 매주" : " · 한 번");
    remove.type = "button"; remove.textContent = "삭제"; remove.disabled = controlSaving; remove.setAttribute("aria-label", dateLabel(schedule.start_ms) + " 예약 삭제");
    remove.addEventListener("click", () => saveControl({mode: recordingControl.mode, schedules: recordingControl.schedules.filter((_, i) => i !== index)}));
    row.append(text, remove); list.append(row);
  });
  if (!data.schedules?.length) list.textContent = "등록된 예약이 없습니다.";
}
async function refreshControl() {
  if (controlSaving) return;
  const stream = byId("record-camera").value, request = ++controlRequest;
  if (!stream) return;
  if (!recordingControl) {
    byId("recording-state").textContent = "확인 중";
    byId("add-schedule").disabled = true;
    document.querySelectorAll("[data-record-mode]").forEach(button => {button.disabled = true;});
  }
  try {
    const data = await jsonRequest("/api/recording/" + encodeURIComponent(stream));
    if (request === controlRequest && stream === byId("record-camera").value) { renderControl(data); byId("add-schedule").disabled = false; }
  } catch (error) {
    if (request !== controlRequest) return;
    recordingControl = null; byId("recording-state").textContent = "확인 실패";
    byId("recording-control-hint").textContent = error.message;
    document.querySelectorAll("[data-record-mode]").forEach(b => { b.disabled = true; });
  }
}
async function saveControl(value) {
  if (controlSaving || !recordingControl) return;
  const stream = byId("record-camera").value;
  controlSaving = true; controlRequest++;
  byId("record-camera").disabled = true; byId("add-schedule").disabled = true;
  renderControl(recordingControl);
  try {
    const data = await jsonRequest("/api/recording/" + encodeURIComponent(stream), {method:"POST", headers:{"Content-Type":"application/json"}, body:JSON.stringify(value)});
    recordingControl = data;
    byId("recording-control-hint").textContent = "녹화 설정을 저장했습니다. 재시작 없이 적용됩니다.";
  } catch (error) { byId("recording-control-hint").textContent = error.message; }
  finally { controlSaving = false; byId("record-camera").disabled = false; byId("add-schedule").disabled = false; renderControl(recordingControl); }
}
byId("record-camera").addEventListener("change", () => { recordingControl = null; refreshControl(); });
document.querySelectorAll("[data-record-mode]").forEach(button => button.addEventListener("click", () => { if (recordingControl) saveControl({mode:button.dataset.recordMode, schedules:recordingControl.schedules || []}); }));
byId("schedule-zone").textContent = "표시 시간대: " + Intl.DateTimeFormat().resolvedOptions().timeZone + " · 반복은 고정 시간 간격입니다.";
byId("schedule-form").addEventListener("submit", event => {
  event.preventDefault();
  const start = new Date(byId("schedule-start").value).getTime(), end = new Date(byId("schedule-end").value).getTime();
  if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start) { byId("recording-control-hint").textContent = "종료 시간을 시작 시간보다 늦게 지정해 주세요."; return; }
  if (!recordingControl) { byId("recording-control-hint").textContent = "카메라 녹화 설정을 먼저 불러와 주세요."; return; }
  saveControl({mode:"auto", schedules:[...(recordingControl.schedules || []), {start_ms:start,end_ms:end,repeat_seconds:Number(byId("schedule-repeat").value) || null}]});
});

function recordingUrl(file) {
  const url = new URL(file.url, location.href);
  if (url.origin !== location.origin || !url.pathname.startsWith("/api/recordings/")) throw new Error("녹화 파일 주소가 올바르지 않습니다.");
  return url;
}
function playRecording(file) {
  let url;
  try { url = recordingUrl(file); } catch (error) { showError(error.message); return; }
  applyLayout(1); endSession(); showError("");
  const current = {mode:"file",stream:file.stream,abort:new AbortController(),frames:0,lastStatsAt:0,lastFrameAt:0,startedAt:performance.now()};
  session = current;
  if ([...ui.stream.options].some(o => o.value === file.stream)) ui.stream.value = file.stream;
  ui["current-stream"].textContent = file.stream;
  ui["transport-caption"].textContent = "RECORDING"; ui["transport-path"].textContent = "MP4 ARCHIVE";
  byId("playback-hint").textContent = "녹화: " + dateLabel(file.start_ms) + " · 영상 재생 막대로 이동할 수 있습니다.";
  setAudioStatus("녹화 파일의 음성 트랙은 카메라 입력에 따라 다릅니다.", true);
  empty("녹화 불러오는 중", dateLabel(file.start_ms)); setStatus("connecting", "녹화 연결 중");
  ui.video.addEventListener("loadeddata", () => { ui.empty.hidden = true; playVideo(current); }, {once:true,signal:current.abort.signal});
  ui.video.addEventListener("error", () => fail(current, "녹화 파일을 재생하지 못했습니다. 다운로드 후 코덱을 지원하는 플레이어에서 확인해 주세요."), {signal:current.abort.signal});
  ui.video.src = url.href;
  current.statsTimer = setInterval(() => receiveStats(current),1000); updateButtons();
}
async function refreshRecordings(more = false) {
  const target = byId("recordings"), request = ++archiveRequest;
  const query = new URLSearchParams();
  if (byId("archive-camera").value) query.set("stream",byId("archive-camera").value);
  const from = byId("archive-from").value ? new Date(byId("archive-from").value).getTime() : null;
  const to = byId("archive-to").value ? new Date(byId("archive-to").value).getTime() : null;
  if ((from !== null && !Number.isFinite(from)) || (to !== null && !Number.isFinite(to)) || (from !== null && to !== null && from > to)) { target.textContent = "검색할 시간 범위를 확인해 주세요."; return; }
  if (from !== null) query.set("from",String(from)); if (to !== null) query.set("to",String(to));
  if (!more || query.toString() !== archiveFilter) { archiveOffset = 0; target.textContent = "녹화 검색 중…"; }
  archiveFilter = query.toString(); query.set("limit","20"); query.set("offset",String(archiveOffset));
  byId("refresh-recordings").disabled = true; byId("more-recordings").disabled = true;
  try {
    const data = await jsonRequest("/api/recordings?" + query);
    if (request !== archiveRequest) return;
    if (!archiveOffset) target.replaceChildren();
    byId("recording-summary").textContent = (data.used_bytes / 1048576).toFixed(1) + " / " + (data.limit_bytes / 1048576).toFixed(0) + " MB";
    for (const file of data.files) {
      const row = document.createElement("article"), title = document.createElement("strong"), meta = document.createElement("span"), actions = document.createElement("div"), play = document.createElement("button"), download = document.createElement("a");
      title.textContent = file.stream + " · " + dateLabel(file.start_ms);
      const duration = Math.max(0,(file.end_ms-file.start_ms)/1000);
      meta.textContent = (file.bytes / 1048576).toFixed(1) + " MB · " + (duration > 0 && duration < 1 ? "1초 미만" : duration.toFixed(0) + "초") + (file.estimated ? " · 시간 추정" : "");
      play.type = "button"; play.textContent = "재생"; play.addEventListener("click", () => playRecording(file));
      const url = recordingUrl(file); url.searchParams.set("download","1"); download.href = url.href; download.download = file.file; download.textContent = "다운로드";
      actions.append(play,download); row.append(title,meta,actions); target.append(row);
    }
    archiveOffset += data.files.length; byId("more-recordings").hidden = data.files.length < 20;
    if (!archiveOffset) target.textContent = "해당 기간에 완료된 녹화가 없습니다. 저장 중인 파일은 완료 후 표시됩니다.";
  } catch (error) { if (request === archiveRequest) target.textContent = error.message; }
  finally { if (request === archiveRequest) { byId("refresh-recordings").disabled = false; byId("more-recordings").disabled = false; } }
}
byId("archive-search").addEventListener("submit", event => { event.preventDefault(); refreshRecordings(); });
byId("more-recordings").addEventListener("click", () => refreshRecordings(true));


// Each camera owns its transport, decoder, timers and retry state.
function selectedCameras() {
  return [...document.querySelectorAll("#camera-choices input:checked")].map(input => input.value);
}
function limitChoices() {
  const count = selectedCameras().length, limit = wallLayout > 1 ? wallLayout : 4;
  for (const input of document.querySelectorAll("#camera-choices input")) input.disabled = !input.checked && count >= limit;
  byId("selection-count").textContent = count + " / " + limit + "대 선택 · 카메라별 자동 재연결";
  updateButtons();
}
function renderCameraChoices() {
  const previous = new Set(selectedCameras());
  const chosen = previous.size ? previous : new Set(cameras.slice(0, Math.min(wallLayout > 1 ? wallLayout : 4, 4)).map(c => c.id));
  const target = byId("camera-choices");
  target.replaceChildren();
  let count = 0;
  for (const camera of cameras) {
    const label = document.createElement("label");
    const input = document.createElement("input");
    input.type = "checkbox"; input.value = camera.id;
    input.checked = chosen.has(camera.id) && count < (wallLayout > 1 ? wallLayout : 4);
    if (input.checked) count++;
    const name = document.createElement("span"); name.textContent = camera.id;
    label.append(input, name);
    input.addEventListener("change", () => { limitChoices(); if (wallActive) reconcileWall(); });
    target.append(label);
  }
  limitChoices();
  if (wallActive) reconcileWall();
}
function scheduleCameraPoll() {
  clearTimeout(cameraPoll);
  cameraPoll = setTimeout(async () => {
    if (!streamsLoaded) return;
    try {
      const response = await fetch("/api/streams", { signal: AbortSignal.timeout(5000) });
      if (!response.ok) throw new Error("status");
      const data = await response.json();
      cameras = data.streams;
      for (const player of wallPlayers.values()) player.recording(cameras.find(c => c.id === player.id));
      refreshAudioCapability();
      refreshControl();
    } catch { for (const player of wallPlayers.values()) player.recording(null); }
    scheduleCameraPoll();
  }, 5000);
}
function wallSummary() {
  if (!wallActive) return;
  const players = [...wallPlayers.values()];
  const live = players.filter(p => p.state === "live").length;
  byId("wall-summary").textContent = live + " / " + players.length + " LIVE";
  setStatus(live === players.length && live > 0 ? "live" : "stale", live + " / " + players.length + "대 수신 중");
}
function placeholders() {
  const wall = byId("camera-wall");
  wall.querySelectorAll(".camera-placeholder").forEach(e => e.remove());
  for (let i = wallPlayers.size; i < wallLayout; i++) {
    const slot = document.createElement("div");
    slot.className = "camera-placeholder";
    const label = document.createElement("span"); label.textContent = String(i + 1).padStart(2, "0");
    const hint = document.createElement("p"); hint.textContent = "카메라 선택 대기";
    slot.append(label, hint); wall.append(slot);
  }
}
function stopWall() {
  wallActive = false;
  for (const player of wallPlayers.values()) player.destroy();
  wallPlayers.clear();
  const wall = byId("camera-wall");
  if (wall) { wall.replaceChildren(); wall.classList.remove("focused"); if (wallLayout > 1) placeholders(); }
  if (byId("wall-summary")) byId("wall-summary").textContent = wallLayout === 1 ? "SINGLE VIEW" : wallLayout + " VIEW";
}
function reconcileWall() {
  const selected = new Set(selectedCameras());
  for (const [id, player] of wallPlayers) {
    if (!selected.has(id)) { player.destroy(); wallPlayers.delete(id); }
  }
  const wall = byId("camera-wall");
  wall.querySelectorAll(".camera-placeholder").forEach(e => e.remove());
  for (const id of selected) {
    if (!wallPlayers.has(id)) {
      const player = new CameraPlayer(id, ui.mode.value);
      wallPlayers.set(id, player); wall.append(player.element); player.connect();
    }
  }
  if (!wall.querySelector(".camera-tile.expanded")) wall.classList.remove("focused");
  placeholders(); wallSummary(); updateButtons();
}
function startWall() {
  if (!selectedCameras().length || !streamsLoaded) return;
  endSession();
  showError(""); wallActive = true; reconcileWall();
}
function applyLayout(value) {
  if (value === wallLayout) return;
  if (wallLayout === 1 || value === 1) endSession();
  wallLayout = value;
  byId("single-stage").hidden = value !== 1;
  byId("wall-stage").hidden = value === 1;
  byId("camera-selection").hidden = value === 1;
  ui.stream.closest("label").hidden = value !== 1;
  ui["stream-hint"].hidden = value !== 1;
  document.querySelector('section[aria-label="수신 상태"]').hidden = value !== 1;
  byId("camera-wall").dataset.layout = String(value);
  document.querySelectorAll("[data-layout]").forEach(e => { if (e.tagName === "BUTTON") e.setAttribute("aria-pressed", String(Number(e.dataset.layout) === value)); });
  if (value > 1) {
    [...document.querySelectorAll("#camera-choices input:checked")].slice(value).forEach(e => { e.checked = false; });
    limitChoices(); if (wallActive) reconcileWall(); else placeholders();
    byId("wall-summary").textContent = value + " VIEW"; if (wallActive) wallSummary();
  } else byId("wall-summary").textContent = "SINGLE VIEW";
}
document.querySelectorAll("button[data-layout]").forEach(button => button.addEventListener("click", () => applyLayout(Number(button.dataset.layout))));
ui.mode.addEventListener("change", () => { if (wallActive) startWall(); });

class CameraPlayer {
  constructor(id, mode) {
    this.id = id; this.mode = mode; this.state = "connecting"; this.generation = 0; this.attempts = 0;
    this.element = document.createElement("article");
    this.element.className = "camera-tile"; this.element.dataset.camera = id;
    this.element.setAttribute("aria-label", id + " 카메라");
    this.element.innerHTML = '<header><strong></strong><span class="camera-rec"></span></header><div class="camera-picture"><video autoplay muted playsinline></video><p class="camera-message" role="status"></p></div><footer><span class="camera-status"></span><span class="camera-stats"></span><span class="camera-audio"></span></footer><div class="camera-actions"><button type="button" class="camera-retry">재연결</button><button type="button" class="camera-expand" aria-pressed="false">확대</button><button type="button" class="camera-sound" aria-pressed="false" disabled>소리 켜기</button></div>';
    this.element.querySelector("strong").textContent = id;
    this.video = this.element.querySelector("video"); this.video.muted = true;
    this.video.setAttribute("aria-label", id + " 실시간 영상");
    this.message = this.element.querySelector(".camera-message");
    this.label = this.element.querySelector(".camera-status");
    this.stats = this.element.querySelector(".camera-stats");
    this.element.querySelector(".camera-retry").addEventListener("click", () => { this.attempts = 0; this.connect(); });
    this.element.querySelector(".camera-expand").addEventListener("click", () => this.expand());
    this.element.querySelector(".camera-sound").addEventListener("click", () => {
      const enable = this.video.muted;
      for (const player of wallPlayers.values()) {
        player.video.muted = true;
        const button = player.element.querySelector(".camera-sound"); button.textContent = "소리 켜기"; button.setAttribute("aria-pressed","false");
      }
      this.video.muted = !enable;
      const button = this.element.querySelector(".camera-sound"); button.textContent = enable ? "소리 끄기" : "소리 켜기"; button.setAttribute("aria-pressed",String(enable));
      if (enable) this.video.play().catch(() => {});
    });
    this.video.addEventListener("dblclick", () => this.expand());
    this.recording(cameras.find(c => c.id === id));
  }
  expand() {
    const open = !this.element.classList.contains("expanded");
    for (const p of wallPlayers.values()) {
      p.element.classList.remove("expanded");
      const b = p.element.querySelector(".camera-expand"); b.textContent = "확대"; b.setAttribute("aria-pressed", "false");
    }
    byId("camera-wall").classList.toggle("focused", open);
    if (open) {
      this.element.classList.add("expanded");
      const b = this.element.querySelector(".camera-expand"); b.textContent = "분할 복귀"; b.setAttribute("aria-pressed", "true");
    }
  }
  recording(camera) {
    const label = this.element.querySelector(".camera-rec");
    const state = camera?.recording;
    label.dataset.state = state || "unknown";
    label.textContent = !camera ? "녹화 상태 확인 중" : !camera.record ? "녹화 꺼짐" : ({recording:"● 녹화 중",storage_full:"저장 한도 도달",retrying:"녹화 재시도",connecting:"녹화 연결 중",stopped:"녹화 중지"}[state] || "녹화 대기");
    const info = audioCapability(camera, this.mode), audioLabel = this.element.querySelector(".camera-audio");
    if (info) {
      audioLabel.textContent = info.state === "warning" ? "음성 확인 필요" : info.available ? "음성 지원" : "음성 없음 / 대기";
      audioLabel.title = info.text; audioLabel.dataset.state = info.state;
      this.element.querySelector(".camera-sound").disabled = !info.available;
    }
  }
  status(state, text) {
    this.state = state; this.element.dataset.state = state; this.label.textContent = text;
    this.message.hidden = state === "live";
    this.message.textContent = text; wallSummary();
  }
  release() {
    this.generation++;
    clearTimeout(this.retry); clearTimeout(this.deadline); clearInterval(this.timer);
    this.abort?.abort();
    this.hls?.destroy(); this.hls = null;
    if (this.ws) { this.ws.onclose = this.ws.onerror = this.ws.onmessage = null; this.ws.close(); this.ws = null; }
    if (this.pc) { this.pc.ontrack = this.pc.onconnectionstatechange = null; this.pc.close(); this.pc = null; }
    this.video.onerror = null; this.video.srcObject = null; this.video.removeAttribute("src"); this.video.load();
    if (this.objectUrl) URL.revokeObjectURL(this.objectUrl);
    this.objectUrl = null; this.media = null; this.buffer = null; this.queue = []; this.queuedBytes = 0;
  }
  destroy() { this.dead = true; this.release(); this.element.remove(); }
  fail(g, text) {
    if (this.dead || g !== this.generation) return;
    this.release();
    const delay = Math.min(30, 2 ** Math.min(++this.attempts, 5));
    this.status("stale", text + " · " + delay + "초 후 재시도");
    this.retry = setTimeout(() => this.connect(), delay * 1000);
  }
  connect() {
    if (this.dead) return;
    warmDvr(this.id);
    this.release(); const g = this.generation;
    this.abort = new AbortController(); this.received = 0; this.lastFrames = 0; this.lastProgress = performance.now();
    this.element.dataset.frames = "0"; this.stats.textContent = this.mode.toUpperCase(); this.status("connecting", "연결 중");
    this.deadline = setTimeout(() => this.fail(g, "연결 시간 초과"), this.mode === "hls" ? 60000 : 25000);
    this.video.onerror = () => this.fail(g, "영상 디코딩 오류");
    this.timer = setInterval(() => {
      if (g !== this.generation) return;
      const quality = this.video.getVideoPlaybackQuality?.();
      const frames = quality ? Math.max(0, quality.totalVideoFrames - quality.droppedVideoFrames) : 0;
      if (frames > this.lastFrames) {
        clearTimeout(this.deadline); this.lastProgress = performance.now(); this.attempts = 0;
        this.status("live", "실시간");
        this.stats.textContent = this.video.videoWidth + "×" + this.video.videoHeight + " · " + frames + " frames";
        this.element.dataset.frames = String(frames);
      } else if (performance.now() - this.lastProgress > (this.mode === "hls" && !this.lastFrames ? 60000 : 12000)) { this.fail(g, "영상 수신 지연"); }
      this.lastFrames = frames;
    }, 1000);
    const url = new URL(this.mode === "webrtc" ? "/webrtc_playnow/randomport/udp/" + encodeURIComponent(this.id) : "/" + encodeURIComponent(this.id), location.href);
    url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
    try {
      if (this.mode === "hls") attachHls(this.video, "/hls/" + encodeURIComponent(this.id) + "/index.m3u8", null, this, text => this.fail(g, text));
      else if (this.mode === "webrtc") this.rtc(g, url);
      else this.mse(g, url);
    } catch { this.fail(g, "브라우저 재생 지원 확인 필요"); }
  }
  rtc(g, url) {
    const pc = this.pc = new RTCPeerConnection();
    pc.addTransceiver("video", {direction:"recvonly"});
    pc.addTransceiver("audio", {direction:"recvonly"});
    const tracks = new MediaStream();
    const codecs = RTCRtpReceiver.getCapabilities("video").codecs.filter(c => c.mimeType.toLowerCase() === "video/h264");
    pc.getTransceivers()[0].setCodecPreferences(codecs);
    const ws = this.ws = new WebSocket(url); let answered = false;
    pc.ontrack = event => {
      if (g !== this.generation) return;
      tracks.addTrack(event.track); if (this.video.srcObject !== tracks) this.video.srcObject = tracks;
      if (event.track.kind === "audio") this.element.querySelector(".camera-sound").disabled = false;
      this.video.play().catch(error => {if (error.name !== "AbortError") this.fail(g, "자동 재생 차단");});
    };
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === "failed" || pc.connectionState === "closed") this.fail(g, "카메라 연결 끊김");
      else if (pc.connectionState === "disconnected") this.status("stale", "연결 복구 대기");
    };
    ws.onmessage = async event => {
      if (g !== this.generation) return;
      try {
        if (typeof event.data === "string" && event.data.startsWith("H264|-|-|")) {
          const offer = await pc.createOffer();
          offer.sdp = offer.sdp.replaceAll("a=extmap-allow-mixed\r\n", "");
          await pc.setLocalDescription(offer);
          await gatherIce(pc, this.abort.signal);
          if (g === this.generation && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({type:"offer",sdp:pc.localDescription.sdp}));
        } else {
          const parts = event.data.split("|-|-|"), answer = JSON.parse(parts[0]);
          if (answer.type !== "answer") throw new Error("협상 실패");
          answered = true;
          await pc.setRemoteDescription(answer);
          if (parts[1]) await pc.addIceCandidate(JSON.parse(parts[1]));
          ws.close();
        }
      } catch { this.fail(g, "영상 협상 실패"); }
    };
    ws.onerror = ws.onclose = () => { if (!answered) this.fail(g, "카메라 연결 대기"); };
  }
  mse(g, url) {
    if (!window.MediaSource) { this.fail(g, "MSE 미지원 브라우저"); return; }
    this.media = new MediaSource(); this.objectUrl = URL.createObjectURL(this.media); this.video.src = this.objectUrl;
    let mime;
    const pump = () => {
      if (g !== this.generation || this.media?.readyState !== "open") return;
      try {
        if (!this.buffer && mime) {
          this.buffer = this.media.addSourceBuffer(mime);
          this.buffer.addEventListener("updateend", pump, {signal:this.abort.signal});
          this.buffer.addEventListener("error", () => this.fail(g, "영상 버퍼 오류"), {signal:this.abort.signal});
        }
        const b = this.buffer; if (!b || b.updating) return;
        if (b.buffered.length) {
          const edge = b.buffered.end(b.buffered.length - 1);
          if (this.video.currentTime < b.buffered.start(0) || edge - this.video.currentTime > 3) this.video.currentTime = Math.max(b.buffered.start(0), edge - 0.5);
          if (this.video.paused && edge > 0.2) this.video.play().catch(() => this.fail(g, "자동 재생 차단"));
          if (this.video.currentTime - b.buffered.start(0) > 13) { b.remove(b.buffered.start(0), this.video.currentTime - 10); return; }
        }
        if (this.queue.length) { const next = this.queue.shift(); this.queuedBytes -= next.byteLength; b.appendBuffer(next); }
      } catch { this.fail(g, "영상 형식 또는 버퍼 오류"); }
    };
    this.media.addEventListener("sourceopen", pump, {once:true,signal:this.abort.signal});
    const ws = this.ws = new WebSocket(url); ws.binaryType = "arraybuffer";
    ws.onmessage = event => {
      if (g !== this.generation) return;
      try {
        const view = new DataView(event.data); let segment;
        if (view.getUint8(0) === 0) throw new Error("source");
        if (view.getUint8(0) === 1) {
          const size = view.getUint16(18,true);
          const xml = new DOMParser().parseFromString(new TextDecoder("utf-16le").decode(event.data.slice(20,20+size)),"text/xml");
          mime = xml.querySelector("mimetypecodec")?.textContent;
          this.element.querySelector(".camera-sound").disabled = !/mp4a|opus|alaw|ulaw/i.test(mime || "");
          if (!mime || !MediaSource.isTypeSupported(mime)) throw new Error("codec");
          segment = event.data.slice(20+size);
        } else if (view.getUint8(0) === 2 && mime) segment = event.data.slice(10);
        else throw new Error("packet");
        if (this.queuedBytes + segment.byteLength > 8*1024*1024 || this.queue.length >= 128) throw new Error("slow");
        this.queue.push(segment); this.queuedBytes += segment.byteLength; pump();
      } catch { this.fail(g, "카메라 영상 대기"); }
    };
    ws.onclose = ws.onerror = () => this.fail(g, "카메라 연결 대기");
  }
}


// Load and play the configured camera on arrival.
loadStreams().then(ready => { if (ready && ui.stream.value) connect(); });
