#!/usr/bin/env python3
"""Bounded Linux recording verification; owns only temporary 8082/18556 servers.

Requires a synthetic H.264 RTSP source at localhost:8554/camera and ffprobe for
read-only validation. No original/proprietary fixture or FFmpeg process is used.
"""
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / os.environ.get("UVMS_RECORDING_EVIDENCE", "evidence/port/recording-verification.json")
BASE = "http://127.0.0.1:8082"
LIMIT = 1048576
WORK = Path(tempfile.mkdtemp(prefix="uvms-recording-test-"))
REPORT = {"work_dir": str(WORK), "limit_bytes": LIMIT, "segment_seconds": 2,
          "binary": os.environ.get("UVMS_TEST_BINARY", "/opt/uvms/uvms-gateway"), "media_engine": "rust-native",
          "scope": "Synthetic H264, single camera; read-only ffprobe validation"}


def request(path):
    req = urllib.request.Request(BASE + path)
    try:
        with urllib.request.urlopen(req, timeout=3) as r:
            return r.status, r.read(), dict(r.headers)
    except urllib.error.HTTPError as e:
        return e.code, e.read(), dict(e.headers)


def api(path):
    status, body, _ = request(path)
    assert status == 200, (path, status)
    return json.loads(body)


def wait_for(predicate, seconds=25):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        result = predicate()
        if result:
            return result
        time.sleep(0.05)
    raise AssertionError("Timed out waiting for recording state")


def start(name):
    folder = WORK / name
    folder.mkdir()
    config = folder / "config.toml"
    config.write_text(f'''bind = "127.0.0.1:8082"
rtsp_bind = "127.0.0.1:18556"
ice_udp_min = 51002
ice_udp_max = 51002
web_root = "/opt/uvms/web"
recording_dir = "{folder}/recordings"
segment_seconds = 2
recording_limit_bytes = {LIMIT}
[streams.camera]
url_env = "CAMERA_RTSP_URL"
record = true
''')
    env = dict(os.environ, UVMS_CONFIG=str(config),
               CAMERA_RTSP_URL="rtsp://127.0.0.1:8554/camera", PATH="/nonexistent")
    with (folder / "server.log").open("wb") as log:
        process = subprocess.Popen([REPORT["binary"]], env=env, stdout=log, stderr=log)
    return process, folder


def healthy(process):
    assert process.poll() is None, "Owned test server exited unexpectedly"
    try:
        return request("/healthz")[0] == 200
    except OSError:
        return False


def stop(process):
    if process.poll() is None:
        process.send_signal(signal.SIGTERM)
        try:
            return process.wait(timeout=12)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)
            raise AssertionError("Graceful shutdown exceeded 12 seconds")
    return process.returncode


def snapshot(folder):
    return {str(p.relative_to(folder)): p.stat().st_size
            for p in sorted((folder / "recordings").rglob("*"))
            if p.is_file() and p.suffix in (".mp4", ".partial")}


def probe(path):
    r = subprocess.run(["/usr/bin/ffprobe", "-v", "error", "-count_frames",
                        "-show_entries", "stream=codec_name,width,height,nb_read_frames,duration:format=duration",
                        "-of", "json", str(path)], capture_output=True, timeout=12)
    assert r.returncode == 0 and not r.stderr.strip(), r.stderr.decode()
    data = json.loads(r.stdout)
    assert int(data["streams"][0]["nb_read_frames"]) > 0, data
    assert float(data["format"]["duration"]) > 0, data
    return data


def children(process):
    path = Path(f"/proc/{process.pid}/task/{process.pid}/children")
    return path.read_text().strip().split()


process = None
try:
    process, folder = start("quota")
    wait_for(lambda: healthy(process), 10)
    wait_for(lambda: api("/api/streams")["streams"][0]["recording"] == "storage_full")
    initial = snapshot(folder)
    states = []
    for _ in range(6):
        time.sleep(1)
        states.append(api("/api/streams")["streams"][0]["recording"])
        assert snapshot(folder) == initial, "Files changed after storage_full"
    listing = api("/api/recordings")
    assert listing["files"] and all(s == "storage_full" for s in states)
    assert 0 < sum(initial.values()) == listing["used_bytes"] <= LIMIT
    assert all(n.endswith(".mp4") for n in initial), initial
    downloaded = []
    for item in listing["files"]:
        status, body, headers = request(item["url"])
        assert status == 200 and len(body) == item["bytes"]
        assert headers["content-type"] == "video/mp4"
        original = folder / "recordings" / item["stream"] / item["file"]
        assert body == original.read_bytes()
        downloaded.append({"file": item["file"], "bytes": len(body),
                           "sha256": hashlib.sha256(body).hexdigest(), "ffprobe": probe(original)})
    child_pids = children(process)
    assert not child_pids
    REPORT["quota"] = {"storage_full_stable_seconds": 6, "states": states,
                       "stable_files": initial, "used_bytes": listing["used_bytes"],
                       "list_without_credentials": 200, "download_without_credentials": 200,
                       "download": 200, "downloads": downloaded,
                       "child_processes": child_pids, "shutdown_exit": stop(process)}
    process = None

    process, folder = start("graceful")
    wait_for(lambda: healthy(process), 10)
    def active_partial():
        files = list((folder / "recordings").rglob("*.partial"))
        return files[0] if files and files[0].stat().st_size > 4096 else None
    partial = wait_for(active_partial, 10)
    partial_bytes = partial.stat().st_size
    exit_code = stop(process)
    process = None
    final = partial.with_suffix(".mp4")
    assert exit_code == 0 and final.exists() and not partial.exists()
    assert not list((folder / "recordings").rglob("*.partial"))
    REPORT["graceful"] = {"partial_observed_bytes": partial_bytes,
                          "final_file": str(final), "final_bytes": final.stat().st_size,
                          "exit_code": exit_code, "remaining_partials": 0, "ffprobe": probe(final)}
    REPORT["passed"] = True
except Exception as exc:
    REPORT["passed"] = False
    REPORT["error"] = str(exc)
    raise
finally:
    if process is not None:
        stop(process)
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(json.dumps(REPORT, indent=2) + "\n")
    print(json.dumps({"passed": REPORT.get("passed"), "output": str(OUTPUT), "work_dir": str(WORK)}))
