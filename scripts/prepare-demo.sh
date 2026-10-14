#!/usr/bin/env bash
# Fetch the pinned RTSP test fixture, not a production gateway dependency.
set -euo pipefail
cd "$(dirname "$0")/.."
case "$(uname -sm)" in
  'Linux x86_64') ;;
  *) printf 'This fixture downloader supports Linux x86_64 only. Install MediaMTX for your platform manually.\n' >&2; exit 1 ;;
esac
mkdir -p test-results/tools/mediamtx
archive=test-results/tools/mediamtx.tar.gz
curl --fail --location --proto '=https' --tlsv1.2 \
  https://github.com/bluenviron/mediamtx/releases/download/v1.21.2/mediamtx_v1.21.2_linux_amd64.tar.gz \
  --output "$archive"
printf '121be6e00397e473a95d83c75f4fcd3819a6af3b0e1a50889cba43ab4fab6a27  %s\n' "$archive" | sha256sum --check
tar xzf "$archive" -C test-results/tools/mediamtx mediamtx LICENSE
printf 'Fixture ready. Run: MEDIAMTX_BIN="$PWD/test-results/tools/mediamtx/mediamtx" bash scripts/demo.sh\n'
