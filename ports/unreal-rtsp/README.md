# unreal-rtsp

Pure Rust RTSP-over-TCP H264 ingest. No FFmpeg dependency, process launch, C/C++ binding, or original Windows DLL dependency. Source implementation follows the recovered native media boundary documented in `../../docs/original-rtsp-native.md` and the original DLL's captured AVCC samples.

The optional audio extension is standards-based reconstruction, not a recovered native audio algorithm. `Client::audio()` describes AAC-LC (`MPEG4-GENERIC`, AAC-hbr, 13/3/3 AU headers), PCMU/PCMA (8 kHz mono), or Opus (48 kHz, RTP channel count 2). `next_media()` yields video and encoded audio; `next_frame()` remains available to video-only callers. AAC multi-AU packets and bounded fragmented AUs are supported, with duplicate/late packets discarded and fragments reset after loss. Unsupported advertised audio is exposed through `audio_warning()` while video remains usable.

Video `timestamp_90k` and audio `timestamp_90k` share an origin established by the first RTP arrival. Each track then advances by its own RTP clock. This starts as an approximate arrival-based synchronization fallback. Once both tracks provide valid RTCP Sender Reports, matching SSRC/NTP/RTP clocks correlate audio to the video timeline. Audio correction slews by at most 1 ms per access unit, preserves monotonic timestamps, rejects stale reports and disagreements above 5 seconds, and leaves raw codec sample counters unchanged. Until both reports arrive, arrival-based accuracy still applies. Do not compare the independent raw audio sample counter with the video counter. The gateway additionally uses Symphonia/libopus/FDK for shared audio conversion; these are gateway dependencies, not dependencies of this RTSP crate.

Synthetic audio integration check: `RTSP_TEST_URL=rtsp://127.0.0.1:18554/aac cargo run --example capture_audio`. The example verifies audio plus video without writing media or printing camera URLs/credentials. The AAC, PCMU, PCMA and Opus synthetic inputs were exercised on 2026-10-14; all yielded monotonically advancing audio timestamps.

```rust,ignore
let mut source = unreal_rtsp::Client::connect("rtsp://127.0.0.1:8554/camera").await?;
let config = source.video().clone(); // SPS, PPS, decoded dimensions
loop {
    let frame = source.next_frame().await?;
    // frame.avcc: four-byte BE length + encoded NAL; no decoding/re-encoding.
    // timestamp_90k is extended/relative; duration_90k comes from next RTP timestamp.
}
```

Implemented: OPTIONS/DESCRIBE/SETUP/PLAY; session/CSeq tracking; TCP interleaved channels including server-selected channels; H264 SDP/SDP-relative control URL; SPS/PPS in SDP or in-band fallback; Basic and Digest MD5/MD5-sess/SHA-256/SHA-256-sess (qop=auth or no-qop); OPTIONS/GET_PARAMETER keepalive; RTP CSRC/extension/padding validation; single NAL, STAP-A and FU-A; timestamp wrap, packet sequence/loss checks; bounded access units; native-style timestamp grouping, AVCC and keyframe SPS/PPS insertion.

Safety differences from native output are explicit: wait for IDR at startup and after discontinuity/loss; reject malformed packets/unsafe sizes; return reconnect-required error on SPS/PPS change instead of silently pairing a frame with stale mux/RTC metadata. A caller must rebuild downstream codec/mux state after reconnect. Camera connections close when Client is dropped. `next_frame` returns an error after 15 seconds without a complete frame. Frame payload bound 8 MiB, NAL count 256; protocol header 32 KiB/body 1 MiB; pending frame queue 128 frames or 32 MiB.

Scope: H264 video with monotonically increasing presentation timestamps, RTSP 1.0 TCP interleaving. UDP RTSP, RTSP TLS, H265, audio, B-frame DTS/PTS reordering, RTCP clock synchronization/receiver reports, full original trick-play/error compatibility are not implemented here. Incoming RTCP channel packets are ignored. Keepalive failure or stream change is surfaced for the service's reconnect loop.

## Native evidence mapping

| Rust path | Recovered original behavior |
|---|---|
| `rtp.rs::payload` | URTSPClient 0x18001e810/0x18001ea40: H264 payload header/FU/aggregation |
| `rtp.rs::finish` |0x18000ff30: flush same-timestamp NALs when next timestamp arrives |
| `rtp.rs::append` |0x1800100b0: four-byte big-endian NAL length |
| Parameter-set injection |0x180010250: insert SPS/PPS pair for keyframe when not both present |
| `h264.rs::dimensions` |0x180007a80: codec setup from SPS/PPS; portable parsed dimensions |
| Session setup |0x180006c70/0x180006cf0/0x180006d70/0x180006e20: OPTIONS/DESCRIBE/SETUP/PLAY |

Supplemental protocol references: [RFC2326](https://www.rfc-editor.org/rfc/rfc2326) and [RFC6184](https://www.rfc-editor.org/rfc/rfc6184). The port is original code based on observed behavior and protocol definitions; Ghidra output is retained separately as analysis evidence.

## Verification

```
cargo test
cargo clippy --all-targets -- -D warnings
RTSP_TEST_URL=rtsp://127.0.0.1:8554/camera cargo run --example capture
```

Linux Ubuntu 24.04/Rust 1.99 validation:

- Unit tests cover packet loss, truncated RTP/STAP, FU reconstruction, timestamp/sequence wrap, SPS dimensions from the original run, digest RFC vector and injection rejection.
- Async integration test runs local Basic and Digest challenge/response sessions, validates method-specific digests, honors channel 4 and verifies authenticated GET_PARAMETER keepalive.
- Native corpus regression repacketizes the original DLL's synthetic IDR/P frames as STAP/FU/single-NAL packets; restored AVCC frames must match original bytes exactly. Fixtures derive from `evidence/runtime/rtsp-original-mta/frame-011.bin` and `frame-012.bin`; they contain generated test video, not original software binaries.
- Real localhost MediaMTX/synthetic H264 input: 350 frames, 24 keyframes,640×360,6000 ticks per frame at 90 kHz; all AVCC lengths and SPS/PPS/IDR checks passed. `capture-validation.log` retains results. FFmpeg was solely the independent synthetic camera publisher; this receiver starts no subprocess.

No real camera credentials or addresses are included in tests/logs.
# HEVC extension (v0.4.0)

The client additionally accepts H265/90000 using RFC7798 single NAL, AP and FU
without DONL or multiple layers. This is a new standards-based extension, not a
claim that the original HEVC implementation was recovered. Call `video_codec()`
before interpreting video metadata. For H265, `Frame.avcc` contains four-byte
length-prefixed HEVC NALs, and `video()` exposes the HEVC SPS/PPS with dimensions
left at zero until the downstream NVIDIA decoder determines them. H265 must
pass through the NVIDIA adapter before entering H264-only consumers. Existing
H264 callers and packet layout are preserved.

Low-delay camera streams without reordered/B-frame RTP timestamps are supported.
Parameter changes cause a reconnect. See `docs/nvidia-transcoding.md` for GPU
limitations and the explicitly outstanding hardware validation.

