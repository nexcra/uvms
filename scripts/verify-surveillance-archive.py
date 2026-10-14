"""Live archive/HLS regression against a dedicated fixture, never production.

Usage: python3 scripts/verify-surveillance-archive.py http://127.0.0.1:8091 camera
The caller owns fixture lifecycle and storage. No camera URL or credential is read.
"""
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

base, camera = sys.argv[1:3]
assert urllib.parse.urlparse(base).hostname in ("localhost", "127.0.0.1")
assert urllib.parse.urlparse(base).port not in (8080, 8088), "use an isolated test service"
camera = urllib.parse.quote(camera, safe="")


def request(path, data=None, headers=None):
    req = urllib.request.Request(base + path, json.dumps(data).encode() if data is not None else None,
                                 {**({"Content-Type": "application/json"} if data is not None else {}), **(headers or {})})
    with urllib.request.urlopen(req, timeout=20) as response:
        return response.status, {k.lower(): v for k, v in response.headers.items()}, response.read()


def api(path, data=None):
    return json.loads(request(path, data)[2])


def until(predicate, seconds=25):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(.25)
    raise AssertionError("condition did not become true")


control = f"/api/recording/{camera}"
previous = api(control)
report = {}
try:
    api(control, {"mode": "off", "schedules": []})
    until(lambda: api(control)["status"] == "off")
    start = int(time.time() * 1000)
    api(control, {"mode": "on", "schedules": []})
    until(lambda: api(control)["status"] == "recording")
    time.sleep(5)
    api(control, {"mode": "off", "schedules": []})
    until(lambda: api(control)["status"] == "off")
    clips = until(lambda: api(f"/api/recordings?stream={camera}&from={start}")["files"])
    clip = clips[0]
    assert clip["end_ms"] > clip["start_ms"] and not clip["estimated"]
    status, headers, body = request(clip["url"], headers={"Range": "bytes=0-63"})
    assert status == 206 and len(body) == 64 and body[4:8] == b"ftyp"
    assert headers["content-range"].startswith("bytes 0-63/")
    status, _, tail = request(clip["url"], headers={"Range": "bytes=-32"})
    assert status == 206 and len(tail) == 32
    try:
        request(clip["url"], headers={"Range": "bytes=999999999999-"})
        raise AssertionError("unsatisfiable range accepted")
    except urllib.error.HTTPError as error:
        assert error.code == 416
    report["manual_recording_range_download"] = True
    begin = int(time.time() * 1000) + 2000
    api(control, {"mode": "auto", "schedules": [{"start_ms": begin, "end_ms": begin + 6000, "repeat_seconds": None}]})
    until(lambda: api(control)["status"] == "recording")
    until(lambda: not api(control)["active"] and api(control)["status"] == "scheduled")
    report["scheduled_start_and_stop"] = True
    text = request(f"/hls/{camera}/index.m3u8")[2].decode()
    assert "#EXT-X-MAP" in text and "#EXT-X-INDEPENDENT-SEGMENTS" in text
    status = api(f"/api/hls/{camera}")
    assert status["ready"] and status["oldest_ms"] < status["newest_ms"]
    dvr = request(f"/hls/{camera}/index.m3u8?from={status['oldest_ms']}")[2].decode()
    assert "#EXT-X-START:TIME-OFFSET=0" in dvr
    for line in dvr.splitlines():
        if line and not line.startswith("#"):
            assert request(f"/hls/{camera}/{line}")[2][4:8] == b"moof"
    report["hls_live_dvr_segments"] = True
    report["clip"] = clip
    print(json.dumps(report, indent=2))
finally:
    api(control, {"mode": previous["mode"], "schedules": previous["schedules"]})
