#!/usr/bin/env python3
"""Pinned official go2rtc, isolated ports, synthetic RTSP only; no Java changes."""
import hashlib, json, pathlib, subprocess, urllib.request

version = 'v1.9.14'
expected = '32d616af226bd731678ffde328b94cfb94e30339bfefc469cfb76323144615a6'
url = f'https://github.com/AlexxIT/go2rtc/releases/download/{version}/go2rtc_linux_amd64'
root = pathlib.Path(__file__).resolve().parents[1]
binary = pathlib.Path('/opt/uvms-go2rtc-bench/go2rtc')
binary.parent.mkdir(parents=True, exist_ok=True)
if not binary.exists() or hashlib.sha256(binary.read_bytes()).hexdigest() != expected:
    data = urllib.request.urlopen(url, timeout=60).read()
    assert hashlib.sha256(data).hexdigest() == expected, 'Release checksum mismatch'
    binary.write_bytes(data)
    binary.chmod(0o755)
address = subprocess.check_output(['hostname', '-I'], text=True).split()[0]
config = pathlib.Path('/etc/uvms-go2rtc-bench.yaml')
config.write_text(f'''modules: [api, rtsp, webrtc]
api:
  listen: "127.0.0.1:8097"
rtsp:
  listen: "127.0.0.1:8567"
webrtc:
  listen: ":50810/udp"
  ice_servers: []
  candidates: ["{address}:50810", "127.0.0.1:50810"]
  filters:
    networks: [udp4]
streams:
  camera: rtsp://127.0.0.1:8554/camera
''')
pathlib.Path('/etc/systemd/system/uvms-go2rtc-bench.service').write_text('''[Unit]
Description=Isolated go2rtc comparison, synthetic source only
[Service]
User=uvms
Group=uvms
ExecStart=/opt/uvms-go2rtc-bench/go2rtc -config /etc/uvms-go2rtc-bench.yaml
NoNewPrivileges=true
LimitNOFILE=16384
InaccessiblePaths=-/usr/bin/ffmpeg -/usr/bin/ffprobe
''')
subprocess.run(['systemctl', 'daemon-reload'], check=True)
report = {'version': version, 'source': url, 'sha256': expected, 'binary_bytes': binary.stat().st_size,
          'http': 8097, 'media_udp': 50810, 'source_rtsp': 'rtsp://127.0.0.1:8554/camera'}
output = root / 'test-results/go2rtc-comparison/release.json'
output.parent.mkdir(parents=True, exist_ok=True)
output.write_text(json.dumps(report, indent=2))
print(json.dumps(report))
