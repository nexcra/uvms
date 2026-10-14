#!/usr/bin/env python3
"""Isolated Rust benchmark against the existing local synthetic camera."""
import json, pathlib, subprocess, sys, time, urllib.request
root=pathlib.Path(__file__).resolve().parents[1]
directory=root/'test-results/rust-optimization/before'
directory.mkdir(parents=True,exist_ok=True)
baseline=directory/'uvms-gateway'
if not baseline.exists():
    baseline.write_bytes(pathlib.Path('/opt/uvms/uvms-gateway').read_bytes())
    baseline.chmod(0o755)
binary=pathlib.Path(sys.argv[1]).resolve(strict=True) if len(sys.argv)>1 else baseline
pathlib.Path('/etc/uvms-rust-opt-bench.toml').write_text('''bind = "127.0.0.1:8091"
rtsp_bind = "127.0.0.1:8561"
web_root = "/opt/uvms/web"
recording_dir = "/var/lib/uvms-rust-opt-bench/recordings"
default_stream = "camera"
max_sessions = 32
ice_udp_min = 50800
ice_udp_max = 50900
segment_seconds = 2
recording_limit_bytes = 16777216
[streams.camera]
url_env = "OPT_TEST_RTSP"
record = false
[streams.camera_two]
url_env = "OPT_TEST_RTSP"
record = false
''')
# Copy the chosen build into a stable service path before starting it.
subprocess.run(['install','-d','/opt/uvms-rust-opt-bench'],check=True)
subprocess.run(['systemctl','stop','uvms-rust-opt-bench'],check=False,stderr=subprocess.DEVNULL)
subprocess.run(['install','-m','755',str(binary),'/opt/uvms-rust-opt-bench/uvms-gateway'],check=True)
pathlib.Path('/etc/systemd/system/uvms-rust-opt-bench.service').write_text('''[Unit]
Description=Rust optimization benchmark, synthetic input only
[Service]
User=uvms
Group=uvms
Environment=UVMS_CONFIG=/etc/uvms-rust-opt-bench.toml
Environment=OPT_TEST_RTSP=rtsp://127.0.0.1:8554/camera
ExecStart=/opt/uvms-rust-opt-bench/uvms-gateway
StateDirectory=uvms-rust-opt-bench
NoNewPrivileges=true
InaccessiblePaths=-/usr/bin/ffmpeg -/usr/bin/ffprobe
''')
subprocess.run(['systemctl','daemon-reload'],check=True)
subprocess.run(['systemctl','start','uvms-rust-opt-bench'],check=True)
for attempt in range(40):
    try:
        with urllib.request.urlopen('http://127.0.0.1:8091/healthz',timeout=1): break
    except OSError:
        if attempt==39: raise
        time.sleep(.25)
print(json.dumps({'service':'uvms-rust-opt-bench','binary':str(binary),'http':8091}))
