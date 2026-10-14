#!/usr/bin/env python3
"""Bounded HLS lifecycle regression using an isolated server and synthetic RTSP.

Usage: python3 scripts/verify-hls-idle.py /absolute/new/binary [--baseline /old/binary]
No live camera settings or installed services are changed. Tests need the existing
synthetic RTSP publisher at 127.0.0.1:8554/camera.
"""
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import time
import urllib.request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary', type=Path)
parser.add_argument('--baseline', type=Path)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
output = root / 'test-results/hls-idle'
output.mkdir(parents=True, exist_ok=True)
base = 'http://127.0.0.1:8095'

def get(path):
    with urllib.request.urlopen(base + path, timeout=45) as response:
        return response.read()

def api(path):
    return json.loads(get(path))

def until(check, seconds):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = check()
        if value:
            return value
        time.sleep(.25)
    raise AssertionError('HLS lifecycle condition timed out')

def inputs(pid):
    inodes = set()
    for fd in Path(f'/proc/{pid}/fd').iterdir():
        try:
            link = os.readlink(fd)
            if link.startswith('socket:['):
                inodes.add(link[8:-1])
        except FileNotFoundError:
            pass
    return sum(1 for table in ['tcp', 'tcp6']
               for line in Path(f'/proc/{pid}/net/{table}').read_text().splitlines()[1:]
               if (fields := line.split())[9] in inodes and fields[3] == '01'
               and int(fields[2].split(':')[1], 16) == 8554)

def run(binary, baseline=False):
    # Refuse to collide with another process; Popen only owns this temporary server.
    for kind, port in [(socket.SOCK_STREAM, 8095), (socket.SOCK_STREAM, 18559), (socket.SOCK_DGRAM, 51500)]:
        with socket.socket(socket.AF_INET, kind) as probe:
            if kind == socket.SOCK_STREAM:
                probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            probe.bind(('0.0.0.0' if kind == socket.SOCK_DGRAM else '127.0.0.1', port))
    with tempfile.TemporaryDirectory(prefix='uvms-hls-idle-') as directory:
        config = Path(directory) / 'config.toml'
        config.write_text(f'''bind = "127.0.0.1:8095"
rtsp_bind = "127.0.0.1:18559"
web_root = "{root}/web"
recording_dir = "{directory}/recordings"
default_stream = "camera"
ice_udp_min = 51500
ice_udp_max = 51500
segment_seconds = 2
hls_segment_seconds = 2
hls_window_seconds = 30
hls_limit_bytes = 8388608
''' + ('' if baseline else 'hls_idle_seconds = 15\n') + '''[streams.camera]
url_env = "UVMS_IDLE_SYNTHETIC_RTSP"
record = false
''')
        log = output / ('baseline.log' if baseline else 'after.log')
        with log.open('w') as stdout:
            process = subprocess.Popen([str(binary.resolve())], stdout=stdout, stderr=subprocess.STDOUT,
                env={**os.environ, 'UVMS_CONFIG': str(config), 'UVMS_IDLE_SYNTHETIC_RTSP': 'rtsp://127.0.0.1:8554/camera'})
            try:
                for _ in range(80):
                    if process.poll() is not None:
                        raise AssertionError('Test server failed to start; inspect isolated log')
                    try:
                        api('/healthz')
                        break
                    except OSError:
                        time.sleep(.1)
                assert inputs(process.pid) == 0
                api('/api/hls/camera')
                time.sleep(1)
                report = {'status_only_inputs': inputs(process.pid)}
                if not baseline:
                    assert report['status_only_inputs'] == 0
                with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
                    playlists = list(pool.map(lambda _: get('/hls/camera/index.m3u8').decode(), range(8)))
                first = int(re.search(r'#EXT-X-MEDIA-SEQUENCE:(\d+)', playlists[0])[1])
                report['playing_inputs'] = inputs(process.pid)
                assert report['playing_inputs'] == 1
                assert all('#EXT-X-MAP:' in p for p in playlists)
                report['active_cache_bytes'] = api('/api/hls/camera')['bytes']
                # Status polling must not keep a playback worker alive.
                for _ in range(18):
                    time.sleep(1)
                    status = api('/api/hls/camera')
                report['cache_after_idle_bytes'] = status['bytes']
                if baseline:
                    report['inputs_after_idle'] = inputs(process.pid)
                    return report
                assert status['bytes'] == 0 and not status['running']
                until(lambda: inputs(process.pid) == 0, 35)
                report['inputs_after_idle'] = 0
                get('/hls/camera/index.m3u8')
                # A recorder keeps the same shared source after HLS becomes idle.
                req = urllib.request.Request(base + '/api/recording/camera',
                    json.dumps({'mode':'on','schedules':[]}).encode(), {'Content-Type':'application/json'})
                with urllib.request.urlopen(req) as response:
                    assert response.status == 200
                for _ in range(18):
                    time.sleep(1)
                    status = api('/api/hls/camera')
                assert not status['running'] and status['bytes'] == 0
                assert inputs(process.pid) == 1
                assert api('/api/recording/camera')['status'] == 'recording'
                report['recording_survives_hls_idle'] = True
                reopened = get('/hls/camera/index.m3u8').decode()
                assert int(re.search(r'#EXT-X-MEDIA-SEQUENCE:(\d+)', reopened)[1]) > first
                assert inputs(process.pid) == 1
                report['restart_uses_new_sequence_and_one_input'] = True
                report['passed'] = True
                return report
            finally:
                process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()

report = {}
if args.baseline:
    report['before'] = run(args.baseline, baseline=True)
    (output / 'baseline.json').write_text(json.dumps(report['before'], indent=2))
report['after'] = run(args.binary)
(output / 'lifecycle.json').write_text(json.dumps(report, indent=2))
print(json.dumps(report, indent=2))
