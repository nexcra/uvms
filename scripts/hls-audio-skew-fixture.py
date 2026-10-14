#!/usr/bin/env python3
"""Isolated HLS regression: synthetic AAC from surveillance-fixture.py only.

Run in Linux/WSL with --binary /absolute/path/uvms-gateway. The fixed upstream
127.0.0.1:18554/aac is the synthetic tone fixture, never a production camera.
Only its audio RTCP Sender Report NTP time is shifted. Media bytes are untouched.
Ctrl-C stops the child gateway and removes this fixture's temporary directory.
"""
import argparse
import asyncio
import contextlib
import json
from pathlib import Path
import tempfile


async def proxy(reader, writer, offset_ms):
    tasks = []
    remote = None
    try:
        upstream, remote = await asyncio.open_connection("127.0.0.1", 18554)

        async def requests():
            while data := await reader.read(65536):
                remote.write(data)
                await remote.drain()

        async def responses():
            while True:
                first = await upstream.readexactly(1)
                if first == b"$":
                    header = await upstream.readexactly(3)
                    body = bytearray(await upstream.readexactly(int.from_bytes(header[1:], "big")))
                    # This fixture negotiates video 0/1 and AAC 2/3, over TCP.
                    if header[0] == 3:
                        at = 0
                        while at + 4 <= len(body):
                            length = (int.from_bytes(body[at+2:at+4], "big") + 1) * 4
                            if length < 4 or at + length > len(body):
                                break
                            if body[at+1] == 200 and length >= 28:
                                ntp = int.from_bytes(body[at+8:at+16], "big")
                                ntp += offset_ms * (1 << 32) // 1000
                                body[at+8:at+16] = ntp.to_bytes(8, "big")
                            at += length
                    writer.write(first + header + body)
                else:
                    header = first + await upstream.readuntil(b"\r\n\r\n")
                    size = next((int(line.split(b":", 1)[1]) for line in header.split(b"\r\n")
                                 if line.lower().startswith(b"content-length:")), 0)
                    if not 0 <= size <= 1024 * 1024:
                        raise ValueError("Oversized fixture RTSP response")
                    writer.write(header + await upstream.readexactly(size))
                await writer.drain()

        tasks = [asyncio.create_task(requests()), asyncio.create_task(responses())]
        await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
    except (OSError, asyncio.IncompleteReadError):
        pass
    finally:
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        writer.close()
        if remote:
            remote.close()


async def main(args):
    import os
    root = Path(__file__).resolve().parents[1]
    binary = Path(args.binary).resolve(strict=True)
    server = await asyncio.start_server(lambda r, w: proxy(r, w, args.offset_ms), "127.0.0.1", 18565)
    with tempfile.TemporaryDirectory(prefix="uvms-hls-audio-") as folder:
        directory = Path(folder)
        config = directory / "config.toml"
        config.write_text(f'''bind="127.0.0.1:8097"
rtsp_bind="127.0.0.1:18561"
web_root="{root}/web"
recording_dir="{directory}/recordings"
default_stream="aac"
ice_udp_min=51700
ice_udp_max=51700
segment_seconds=2
hls_segment_seconds=2
hls_window_seconds=30
hls_limit_bytes=8388608
hls_idle_seconds=15
[streams.aac]
url_env="UVMS_SKEW_AAC_SOURCE"
record=false
''')
        with (directory / "gateway.log").open("w") as log:
            child = await asyncio.create_subprocess_exec(str(binary), stdout=log, stderr=log,
                env={**os.environ, "UVMS_CONFIG": str(config),
                     "UVMS_SKEW_AAC_SOURCE": "rtsp://127.0.0.1:18565/aac"})
            try:
                await asyncio.sleep(1)
                if child.returncode is not None:
                    raise RuntimeError((directory / "gateway.log").read_text())
                print(json.dumps({"url": "http://127.0.0.1:8097", "offset_ms": args.offset_ms,
                                  "synthetic_only": True}), flush=True)
                async with server:
                    await child.wait()
            finally:
                server.close()
                await server.wait_closed()
                if child.returncode is None:
                    child.terminate()
                    try:
                        await asyncio.wait_for(child.wait(), 10)
                    except asyncio.TimeoutError:
                        child.kill()
                        await child.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--offset-ms", type=int, default=-4000, choices=range(-4000, 4001), metavar="[-4000..4000]")
    with contextlib.suppress(KeyboardInterrupt):
        asyncio.run(main(parser.parse_args()))
