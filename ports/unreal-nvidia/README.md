# Direct NVIDIA adapter

New extension, not a recovered Unreal GPU algorithm. `bridge.cpp` calls NVIDIA's
CUDA Driver API, NVDEC parser/decoder and NVENC H264 encoder directly. No FFmpeg,
libavcodec, external transcoder process, or CPU video fallback is used.

The checked-in codec headers are unmodified NVIDIA MIT-licensed files, with
source commit and SHA256 in `headers.toml`. CUDA development headers are supplied
by a CUDA Toolkit installation or `scripts/prepare-nvidia-headers.py`; they are
not bundled with the source archive. The helper downloads only headers/license
from NVIDIA's pinned PyPI wheel, checks SHA256, and installs no driver or runtime.

Build with `--features nvidia`, setting `CUDA_INCLUDE_DIR` if necessary. Driver
libraries (`libcuda.so.1`, `libnvcuvid.so.1`, `libnvidia-encode.so.1`) are loaded
with `dlopen` only when requested. The binary can run ordinary H264 streams on a
host without an NVIDIA driver. A GPU feature-disabled build rejects HEVC explicitly.

`Transcoder` is deliberately neither Send nor Sync. Each camera uses one worker
thread for context creation, parsing, synchronous encode and destruction. No C++
exception crosses a C callback or Rust FFI boundary. Decoded NV12 surfaces are
registered directly with NVENC and remain mapped until encode completion.
Only compressed HEVC input and H264 output cross the CPU/GPU boundary.

Scope: progressive HEVC Main 8-bit 4:2:0, no B-frame/reordered RTP timestamps,
single RTP layer and no DONL, output H264 baseline with no B frames/lookahead.
No scaling, Main10 conversion, interlacing or GPU inference is implemented.
Resolution/parameter changes terminate the source so the existing reconnect
path creates a new decoder/encoder. Normal live shutdown discards queued tail
pictures; this adapter is not an offline lossless file converter.

The initial implementation has compiled against real NVIDIA headers, but the
development host has AMD integrated graphics and no NVIDIA driver. Actual GPU
conversion, NVENC quality, long-duration stability, latency and GPU concurrency
are **not yet hardware-validated**. See `docs/nvidia-transcoding.md` for the gate.
