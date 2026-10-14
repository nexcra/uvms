#!/usr/bin/env python3
"""Owns only uvms-soak* test units, synthetic sources and isolated recording data."""
import argparse
import asyncio
import hashlib
import json
import pathlib
import subprocess
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
CONFIG = pathlib.Path('/etc/uvms-soak')
ALIASES = ['aac', 'pcmu', 'pcma', 'opus']
UNITS = ['uvms-soak', 'uvms-soak-proxy', 'uvms-soak-rtsp'] + ['uvms-soak-source-' + a for a in ALIASES]


async def proxy(reader, writer):
    remote = None
    tasks = []
    try:
        first = await asyncio.wait_for(reader.readuntil(b'\r\n\r\n'), 10)
        target = first.split(b' ', 2)[1].decode('ascii')
        alias = target.split('/')[-1].split('?')[0]
        if alias not in ALIASES:
            return
        upstream, remote = await asyncio.open_connection('127.0.0.1', 18574)
        blocked = lambda: (CONFIG / ('blackhole-' + alias)).exists()
        if not blocked():
            remote.write(first)
            await remote.drain()

        async def copy(source, destination):
            while data := await source.read(65536):
                # Drop bytes while keeping TCP established: exercise a silent network outage.
                if not blocked():
                    destination.write(data)
                    await destination.drain()

        tasks = [asyncio.create_task(copy(reader, remote)), asyncio.create_task(copy(upstream, writer))]
        await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
    except (OSError, ValueError, IndexError, asyncio.IncompleteReadError, asyncio.TimeoutError):
        pass
    finally:
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        writer.close()
        if remote:
            remote.close()


async def serve():
    server = await asyncio.start_server(proxy, '127.0.0.1', 18576)
    async with server:
        await server.serve_forever()


def systemctl(*args):
    subprocess.run(['systemctl', *args], check=True)


def setup(binary):
    CONFIG.mkdir(exist_ok=True)
    for alias in ALIASES:
        (CONFIG / ('blackhole-' + alias)).unlink(missing_ok=True)
    subprocess.run(['systemctl', 'stop', *UNITS], check=False, stderr=subprocess.DEVNULL)
    destination = pathlib.Path('/opt/uvms-soak/uvms-gateway')
    destination.parent.mkdir(exist_ok=True)
    subprocess.run(['install', '-m', '755', str(pathlib.Path(binary).resolve(strict=True)), str(destination)], check=True)
    run = str(time.time_ns())
    recordings = '/var/lib/uvms-soak/recordings-' + run
    (CONFIG / 'mediamtx.yml').write_text('''logLevel: warn
rtspAddress: 127.0.0.1:18574
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
        pathlib.Path('/etc/systemd/system/' + name + '.service').write_text(f'''[Unit]
Description=UVMS isolated soak test {name}
[Service]
User=uvms
Group=uvms
ExecStart={command}
NoNewPrivileges=true
LimitNOFILE=16384
{extra}
''')

    unit('uvms-soak-rtsp', '/opt/uvms-test/mediamtx /etc/uvms-soak/mediamtx.yml')
    unit('uvms-soak-proxy', f'/usr/bin/python3 {ROOT}/scripts/soak-fixture.py proxy')
    for i, (alias, codec, rate, channels) in enumerate([
        ('aac', 'aac', 48000, 2), ('pcmu', 'pcm_mulaw', 8000, 1),
        ('pcma', 'pcm_alaw', 8000, 1), ('opus', 'libopus', 48000, 2)
    ]):
        unit('uvms-soak-source-' + alias,
             f'/usr/bin/ffmpeg -hide_banner -loglevel error -re -f lavfi -i testsrc2=size=640x360:rate=15 '
             f'-f lavfi -i sine=frequency={440+i*110}:sample_rate={rate} -c:v libx264 -threads 1 '
             f'-preset ultrafast -tune zerolatency -profile:v baseline -pix_fmt yuv420p -g 15 -bf 0 '
             f'-x264-params repeat-headers=1 -c:a {codec} -ar {rate} -ac {channels} '
             f'-f rtsp -rtsp_transport tcp rtsp://127.0.0.1:18574/{alias}', 'Restart=on-failure\nRestartSec=2')
    config = f'''bind="127.0.0.1:8099"
rtsp_bind="127.0.0.1:18575"
web_root="{ROOT}/web"
recording_dir="{recordings}"
default_stream="aac"
max_sessions=64
ice_udp_min=51900
ice_udp_max=51900
segment_seconds=15
recording_limit_bytes=8589934592
hls_segment_seconds=2
hls_window_seconds=30
hls_limit_bytes=8388608
hls_idle_seconds=15
'''
    environment = 'Environment=UVMS_CONFIG=/etc/uvms-soak/config.toml\n'
    for alias in ALIASES:
        config += f'[streams.{alias}]\nurl_env="SOAK_{alias.upper()}"\nrecord=false\n'
        environment += f'Environment=SOAK_{alias.upper()}=rtsp://127.0.0.1:18576/{alias}\n'
    (CONFIG / 'config.toml').write_text(config)
    unit('uvms-soak', str(destination), environment + 'StateDirectory=uvms-soak\nInaccessiblePaths=-/usr/bin/ffmpeg -/usr/bin/ffprobe')
    systemctl('daemon-reload')
    systemctl('start', 'uvms-soak-rtsp', 'uvms-soak-proxy')
    systemctl('start', *['uvms-soak-source-' + a for a in ALIASES])
    systemctl('start', 'uvms-soak')
    for _ in range(50):
        try:
            with urllib.request.urlopen('http://127.0.0.1:8099/healthz', timeout=1):
                break
        except OSError:
            time.sleep(.2)
    else:
        raise RuntimeError('Soak fixture did not start')
    report = {'run': run, 'recording_dir': recordings, 'binary_sha256': hashlib.sha256(destination.read_bytes()).hexdigest(),
              'binary_bytes': destination.stat().st_size, 'aliases': ALIASES, 'http': 8099, 'source_proxy_port': 18576}
    (CONFIG / 'run.json').write_text(json.dumps(report, indent=2))
    print(json.dumps(report))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['setup', 'proxy', 'stop', 'source-off', 'source-on', 'blackhole', 'restore', 'restart-inputs', 'restart-gateway', 'info'])
    parser.add_argument('--binary', default='/opt/uvms/uvms-gateway')
    parser.add_argument('--camera', choices=ALIASES, default='aac')
    args = parser.parse_args()
    if args.action == 'setup':
        setup(args.binary)
    elif args.action == 'proxy':
        asyncio.run(serve())
    elif args.action == 'stop':
        systemctl('stop', *UNITS)
    elif args.action in ['source-off', 'source-on']:
        systemctl('stop' if args.action == 'source-off' else 'start', 'uvms-soak-source-' + args.camera)
    elif args.action == 'blackhole':
        (CONFIG / ('blackhole-' + args.camera)).touch()
    elif args.action == 'restore':
        (CONFIG / ('blackhole-' + args.camera)).unlink(missing_ok=True)
    elif args.action == 'restart-inputs':
        systemctl('restart', 'uvms-soak-rtsp')
    elif args.action == 'restart-gateway':
        systemctl('restart', 'uvms-soak')
    elif args.action == 'info':
        print((CONFIG / 'run.json').read_text())
