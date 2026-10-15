# Third-party components

This Rust implementation follows the analyzed native media flow and SDK wire protocol.
The supplied Unreal Media Server installer was reverse engineered locally; its binaries, private keys,
certificates, and SDK JavaScript are not dependencies of the gateway or included in
the deliverable archives.

- Rust dependencies, including webrtc-rs, Axum and Tokio, are pinned by `Cargo.lock`.
  Consult each crate's license before redistributing a product built from this project.
- FFmpeg is used only to generate the synthetic test camera. It is not a service
  runtime dependency. ffprobe is used outside the service for development validation.
  Neither executable is bundled in the native binary archive.
- Playwright is a development-only browser verification dependency.
- ONVIF uses reqwest 0.12.28 (MIT/Apache-2.0) with rustls TLS, roxmltree 0.21.1
  (MIT/Apache-2.0) for bounded XML parsing, and sha1 0.10 (MIT/Apache-2.0) for
  the standard WS-Security UsernameToken digest. HTTP Digest reuses this project's
  RTSP authentication implementation. No vendor SDK or external ONVIF daemon is used.
  This is a standards-based extension, not a recovered uVMS ONVIF implementation.
- hls.js 1.7.3 (Apache-2.0) is vendored for browser HLS playback, including its
  license in `web/vendor/hls.LICENSE`; no CDN is required at runtime.
- Audio uses Symphonia 0.5.5 (MPL-2.0), standalone libopus via opus 0.3.1 /
  audiopus_sys 0.2.2, and standalone FDK AAC via fdk-aac 0.8.0 / fdk-aac-sys 0.5.0.
  These are codec libraries linked into the Rust service, not FFmpeg processes
  or libavcodec. AAC is preserved for MP4/HLS and decoded/re-encoded to Opus for
  WebRTC; G.711/Opus remain unchanged for WebRTC/RTSP and are converted to AAC
  for MP4/HLS/MSE. Source packages are included unmodified under
  `third-party-sources/` in the source ZIP. FDK and Opus notices are in `licenses/`.
- SQLite via rusqlite 0.32.1 / bundled libsqlite3 stores recording metadata and
  recording schedules locally; no database server is required.
- Optional H265 conversion calls NVIDIA CUDA Driver API, NVDEC and NVENC directly
  through the project's C++ ABI adapter. NVIDIA's unmodified MIT codec API headers
  are included with provenance in `ports/unreal-nvidia/headers.toml`. CUDA development
  headers are obtained separately from the Toolkit or NVIDIA's pinned PyPI wheel;
  neither CUDA headers, CUDA runtime nor NVIDIA driver binaries are bundled here.
  Driver libraries load on demand. No FFmpeg/libavcodec implementation is involved.
- MediaMTX v1.21.2 is used only as a synthetic RTSP-camera fixture. The preparation
  script retrieves its official archive and checks SHA-256. It is not required
  for real camera input and is not included in the application binary archive.

Protocol references: [Unreal SDK](https://umediaserver.net/umediaserver/source.html),
[WebRTC Rust](https://github.com/webrtc-rs/webrtc),
[FFmpeg](https://ffmpeg.org/documentation.html),
[MediaMTX](https://github.com/bluenviron/mediamtx).
