#!/usr/bin/env bash
# Run against a local test-only RTSP publisher endpoint (e.g. MediaMTX).
# FFmpeg and MediaMTX are verification tools; MediaMTX is not a gateway dependency.
set -euo pipefail
exec ffmpeg -hide_banner -loglevel warning -re \
  -f lavfi -i 'testsrc2=size=640x360:rate=15' \
  -an -c:v libx264 -preset ultrafast -tune zerolatency \
  -profile:v baseline -pix_fmt yuv420p -g 15 -bf 0 \
  -x264-params repeat-headers=1 \
  -f rtsp -rtsp_transport tcp "${UVMS_TEST_RTSP_URL:-rtsp://127.0.0.1:8554/camera}"
