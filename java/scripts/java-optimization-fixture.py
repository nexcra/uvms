#!/usr/bin/env python3
"""Isolated Java benchmark against the existing local synthetic RTSP fixture.
No camera credentials or production configuration are read or changed.
Usage: python3 scripts/java-optimization-fixture.py /absolute/path/to/jar
"""
import json, pathlib, subprocess, sys, time, urllib.request

jar = pathlib.Path(sys.argv[1]).resolve(strict=True)
config = {
    'bind': '127.0.0.1', 'port': 8090, 'rtsp_port': 8560,
    'web_root': '/opt/uvms-java/web',
    'recording_dir': '/var/lib/uvms-java-opt-bench/recordings',
    'default_stream': 'camera', 'max_sessions': 32,
    'ice_udp_min': 50600, 'ice_udp_max': 50700,
    'streams': {'camera': {'url_env': 'OPT_TEST_RTSP', 'record': False},
                'camera_two': {'url_env': 'OPT_TEST_RTSP', 'record': False}}
}
pathlib.Path('/etc/uvms-java-opt-bench.json').write_text(json.dumps(config))
unit = f'''[Unit]
Description=Bounded Java optimization benchmark, synthetic input only
[Service]
User=uvms
Group=uvms
Environment=OPT_TEST_RTSP=rtsp://127.0.0.1:8554/camera
Environment=UVMS_JAVA_CONFIG=/etc/uvms-java-opt-bench.json
ExecStart=/usr/bin/java -Djava.net.preferIPv4Stack=true -Xms64m -Xmx512m -jar {jar}
StateDirectory=uvms-java-opt-bench
NoNewPrivileges=true
InaccessiblePaths=-/usr/bin/ffmpeg -/usr/bin/ffprobe
SuccessExitStatus=143
'''
pathlib.Path('/etc/systemd/system/uvms-java-opt-bench.service').write_text(unit)
subprocess.run(['systemctl', 'daemon-reload'], check=True)
subprocess.run(['systemctl', 'restart', 'uvms-java-opt-bench'], check=True)
for attempt in range(40):
    try:
        with urllib.request.urlopen('http://127.0.0.1:8090/healthz', timeout=1): break
    except OSError:
        if attempt == 39: raise
        time.sleep(0.25)
print(json.dumps({'service': 'uvms-java-opt-bench', 'jar': str(jar), 'fixture': '640x360 15fps H264', 'http': 8090}))
