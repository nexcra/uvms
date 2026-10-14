#!/usr/bin/env python3
"""Add a second independent synthetic RTSP fixture to the existing local demo."""
from pathlib import Path
import subprocess
yaml=Path("/etc/uvms-test/mediamtx.yml")
text=yaml.read_text()
if "  camera_two:" not in text: yaml.write_text(text+"\n  camera_two:\n    source: publisher\n")
unit=Path("/etc/systemd/system/uvms-demo-source-two.service")
unit.write_text("""[Unit]
Description=UVMS second independent test camera generator
After=uvms-demo-rtsp.service
Requires=uvms-demo-rtsp.service
[Service]
User=uvms
Group=uvms
ExecStart=/usr/bin/ffmpeg -hide_banner -loglevel warning -re -f lavfi -i testsrc=size=640x360:rate=10 -an -c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline -pix_fmt yuv420p -g 10 -bf 0 -x264-params repeat-headers=1 -f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/camera_two
Restart=always
RestartSec=2
NoNewPrivileges=true
[Install]
WantedBy=multi-user.target
""")
env=Path("/etc/uvms/uvms.env")
if "CAMERA_TWO_RTSP_URL=" not in env.read_text():
    with env.open("a") as f:f.write("\nCAMERA_TWO_RTSP_URL=rtsp://127.0.0.1:8554/camera_two\n")
config=Path("/etc/uvms/config.toml")
text=config.read_text()
if "[streams.camera_two]" not in text:text+='\n[streams.camera_two]\nurl_env = "CAMERA_TWO_RTSP_URL"\nrecord = true\n'
config.write_text(text)
subprocess.run(["systemctl","daemon-reload"],check=True)
subprocess.run(["systemctl","restart","uvms-demo-rtsp","uvms-demo-source"],check=True)
subprocess.run(["systemctl","enable","--now","uvms-demo-source-two"],check=True)
subprocess.run(["systemctl","restart","uvms"],check=True)
