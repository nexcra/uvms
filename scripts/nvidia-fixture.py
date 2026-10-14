"""Isolated synthetic HEVC test source and gateway; never change real cameras."""
import pathlib, subprocess, sys, time, urllib.request
root=pathlib.Path(__file__).resolve().parents[1]
binary=pathlib.Path(sys.argv[1]).resolve(strict=True)
subprocess.run(['systemctl','start','uvms-surveillance-rtsp','uvms-surveillance-source-aac'],check=True)
units=pathlib.Path('/etc/systemd/system')
(units/'uvms-nvidia-source.service').write_text('''[Unit]
Description=UVMS synthetic HEVC camera for GPU verification
[Service]
User=uvms
Group=uvms
ExecStart=/usr/bin/ffmpeg -hide_banner -loglevel error -re -f lavfi -i testsrc2=size=640x360:rate=15 -f lavfi -i sine=frequency=440:sample_rate=48000 -c:v libx265 -preset ultrafast -pix_fmt yuv420p -x265-params bframes=0:keyint=15:min-keyint=15:scenecut=0:repeat-headers=1:pools=1:frame-threads=1 -c:a aac -ar 48000 -ac 2 -f rtsp -rtsp_transport tcp rtsp://127.0.0.1:18554/hevc
Restart=on-failure
RestartSec=2
NoNewPrivileges=true
''')
subprocess.run(['systemctl','stop','uvms-nvidia-test'],check=False,stderr=subprocess.DEVNULL)
subprocess.run(['install','-d','/opt/uvms-nvidia-test','/etc/uvms-nvidia-test'],check=True)
subprocess.run(['install','-m','755',str(binary),'/opt/uvms-nvidia-test/uvms-gateway'],check=True)
pathlib.Path('/etc/uvms-nvidia-test/config.toml').write_text(f'''bind="127.0.0.1:8093"
rtsp_bind="127.0.0.1:18557"
web_root="{root}/web"
recording_dir="/var/lib/uvms-nvidia-test/recordings"
default_stream="h264"
ice_udp_min=51300
ice_udp_max=51300
segment_seconds=2
recording_limit_bytes=67108864
hls_window_seconds=30
hls_limit_bytes=8388608
[streams.h264]
url_env="GPU_TEST_H264"
record=false
[streams.h265]
url_env="GPU_TEST_HEVC"
record=false
''')
(units/'uvms-nvidia-test.service').write_text('''[Unit]
Description=UVMS isolated NVIDIA integration test
[Service]
User=uvms
Group=uvms
Environment=UVMS_CONFIG=/etc/uvms-nvidia-test/config.toml
Environment=GPU_TEST_H264=rtsp://127.0.0.1:18554/aac
Environment=GPU_TEST_HEVC=rtsp://127.0.0.1:18554/hevc
ExecStart=/opt/uvms-nvidia-test/uvms-gateway
StateDirectory=uvms-nvidia-test
InaccessiblePaths=-/usr/bin/ffmpeg -/usr/bin/ffprobe
NoNewPrivileges=true
Restart=on-failure
''')
subprocess.run(['systemctl','daemon-reload'],check=True)
subprocess.run(['systemctl','start','uvms-nvidia-source','uvms-nvidia-test'],check=True)
for _ in range(50):
    try:
        with urllib.request.urlopen('http://127.0.0.1:8093/healthz',timeout=1) as r:
            print(r.read().decode());break
    except OSError:time.sleep(.2)
else:raise RuntimeError('NVIDIA fixture gateway did not start')
