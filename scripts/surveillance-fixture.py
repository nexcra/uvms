#!/usr/bin/env python3
"""Separate synthetic A/V cameras for surveillance verification; no live camera changes."""
import json, pathlib, subprocess, sys, time, urllib.request
root=pathlib.Path(__file__).resolve().parents[1]
pathlib.Path('/etc/uvms-surveillance').mkdir(exist_ok=True)
pathlib.Path('/etc/uvms-surveillance/mediamtx.yml').write_text('''logLevel: warn
rtsp: true
rtspAddress: 127.0.0.1:18554
rtspTransports: [tcp]
rtmp: false
hls: false
webrtc: false
srt: false
moq: false
paths:
  all_others:
    source: publisher
''')
def unit(name, command, extra=''):
    pathlib.Path('/etc/systemd/system/'+name+'.service').write_text(f'''[Unit]
Description=UVMS isolated surveillance test fixture {name}
[Service]
User=uvms
Group=uvms
{extra}
ExecStart={command}
Restart=on-failure
RestartSec=2
NoNewPrivileges=true
''')
unit('uvms-surveillance-rtsp','/opt/uvms-test/mediamtx /etc/uvms-surveillance/mediamtx.yml')
for name, codec, rate, channels in [('aac','aac',48000,2),('pcmu','pcm_mulaw',8000,1),('pcma','pcm_alaw',8000,1),('opus','libopus',48000,2)]:
    unit('uvms-surveillance-source-'+name,f'/usr/bin/ffmpeg -hide_banner -loglevel error -re -f lavfi -i testsrc2=size=640x360:rate=15 -f lavfi -i sine=frequency=440:sample_rate={rate} -c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline -pix_fmt yuv420p -g 15 -bf 0 -x264-params repeat-headers=1 -c:a {codec} -ar {rate} -ac {channels} -f rtsp -rtsp_transport tcp rtsp://127.0.0.1:18554/{name}')
subprocess.run(['systemctl','daemon-reload'],check=True)
subprocess.run(['systemctl','start','uvms-surveillance-rtsp'],check=True)
subprocess.run(['systemctl','start',*[f'uvms-surveillance-source-{name}' for name in ['aac','pcmu','pcma','opus']]],check=True)
if len(sys.argv)>1:
    binary=pathlib.Path(sys.argv[1]).resolve(strict=True)
    subprocess.run(['systemctl','stop','uvms-surveillance'],check=False,stderr=subprocess.DEVNULL)
    subprocess.run(['install','-d','/opt/uvms-surveillance'],check=True)
    subprocess.run(['install','-m','755',str(binary),'/opt/uvms-surveillance/uvms-gateway'],check=True)
    config=f'''bind = "127.0.0.1:8092"
rtsp_bind = "127.0.0.1:18555"
web_root = "{root}/web"
recording_dir = "/var/lib/uvms-surveillance/recordings"
default_stream = "aac"
max_sessions = 32
ice_udp_min = 51200
ice_udp_max = 51200
segment_seconds = 2
recording_limit_bytes = 134217728
hls_segment_seconds = 2
hls_window_seconds = 30
hls_limit_bytes = 8388608
'''
    config+=''.join(f'[streams.{name}]\nurl_env = "TEST_{name.upper()}"\nrecord = false\n' for name in ['aac','pcmu','pcma','opus'])
    pathlib.Path('/etc/uvms-surveillance/config.toml').write_text(config)
    environment='\n'.join(f'Environment=TEST_{name.upper()}=rtsp://127.0.0.1:18554/{name}' for name in ['aac','pcmu','pcma','opus'])
    unit('uvms-surveillance','/opt/uvms-surveillance/uvms-gateway','Environment=UVMS_CONFIG=/etc/uvms-surveillance/config.toml\n'+environment+'\nStateDirectory=uvms-surveillance\nInaccessiblePaths=-/usr/bin/ffmpeg -/usr/bin/ffprobe')
    subprocess.run(['systemctl','daemon-reload'],check=True)
    subprocess.run(['systemctl','start','uvms-surveillance'],check=True)
    for attempt in range(50):
        try:
            with urllib.request.urlopen('http://127.0.0.1:8092/healthz',timeout=1):break
        except OSError:
            if attempt==49:raise
            time.sleep(.2)
print(json.dumps({'fixture_rtsp_port':18554,'http':8092,'aliases':['aac','pcmu','pcma','opus'],'synthetic_only':True}))
