#!/usr/bin/env bash
# Local synthetic-camera fixture. Not part of the UVMS production media engine.
set -euo pipefail
cd "$(dirname "$0")/.."
install -d /opt/uvms-test /etc/uvms-test
install -m 755 /root/uvms-test/mediamtx /opt/uvms-test/mediamtx
install -m 644 deploy/mediamtx-test.yml /etc/uvms-test/mediamtx.yml
cat > /etc/systemd/system/uvms-demo-rtsp.service <<'UNIT'
[Unit]
Description=UVMS test-only synthetic RTSP fixture server
After=network.target
[Service]
User=uvms
Group=uvms
WorkingDirectory=/var/lib/uvms
ExecStart=/opt/uvms-test/mediamtx /etc/uvms-test/mediamtx.yml
Restart=on-failure
NoNewPrivileges=true
[Install]
WantedBy=multi-user.target
UNIT
cat > /etc/systemd/system/uvms-demo-source.service <<'UNIT'
[Unit]
Description=UVMS test-only FFmpeg synthetic camera generator
After=uvms-demo-rtsp.service
Requires=uvms-demo-rtsp.service
[Service]
User=uvms
Group=uvms
ExecStart=/usr/bin/ffmpeg -hide_banner -loglevel warning -re -f lavfi -i testsrc2=size=640x360:rate=15 -an -c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline -pix_fmt yuv420p -g 15 -bf 0 -x264-params repeat-headers=1 -f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/camera
Restart=always
RestartSec=2
NoNewPrivileges=true
[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl start uvms-demo-rtsp uvms-demo-source
