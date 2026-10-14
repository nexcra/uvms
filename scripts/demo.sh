#!/usr/bin/env bash
# Linux-only local synthetic-camera demonstration. Ctrl-C stops every owned process.
set -euo pipefail
cd "$(dirname "$0")/.."
repo="$PWD"
gateway="${UVMS_BIN:-$repo/target/release/uvms-gateway}"
mediamtx="${MEDIAMTX_BIN:-mediamtx}"
lab=$(mktemp -d -t uvms-demo.XXXXXX)
pids=()
cleanup() {
  trap - EXIT INT TERM
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  for pid in "${pids[@]}"; do wait "$pid" 2>/dev/null || true; done
  # Preserve logs for inspection; no original camera credentials are used.
  printf 'Demo stopped. Logs: %s\n' "$lab"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
export CAMERA_RTSP_URL=rtsp://127.0.0.1:8554/camera
export UVMS_CONFIG="$lab/config.toml"
cat > "$UVMS_CONFIG" <<EOF
bind = "127.0.0.1:8080"
rtsp_bind = "127.0.0.1:8555"
web_root = "$repo/web"
recording_dir = "$lab/recordings"
segment_seconds = 10
recording_limit_bytes = 268435456
max_sessions = 8
[streams.camera]
url_env = "CAMERA_RTSP_URL"
record = true
[streams.camera_copy]
url_env = "CAMERA_RTSP_URL"
record = false
EOF
"$mediamtx" "$repo/deploy/mediamtx-test.yml" > "$lab/mediamtx.log" 2>&1 &
pids+=("$!")
sleep 1
bash "$repo/scripts/test-source.sh" > "$lab/source.log" 2>&1 &
pids+=("$!")
sleep 2
for pid in "${pids[@]}"; do kill -0 "$pid" || { cat "$lab/mediamtx.log" "$lab/source.log"; exit 1; }; done
"$gateway" > "$lab/gateway.log" 2>&1 &
pids+=("$!")
for attempt in {1..30}; do
  if curl --silent --fail http://127.0.0.1:8080/healthz > /dev/null; then
    printf 'Demo ready: http://127.0.0.1:8080; logs: %s\n' "$lab"
    break
  fi
  kill -0 "${pids[-1]}" || { cat "$lab/gateway.log"; exit 1; }
  sleep 1
done
wait -n "${pids[@]}"
