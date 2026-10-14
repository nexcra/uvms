#!/usr/bin/env python3
"""Bounded local demo lifecycle verification, uses only named UVMS test units."""
import json, os, subprocess, time, urllib.request
from pathlib import Path
def command(*args): return subprocess.check_output(args,text=True).strip()
def get(path):
    r=urllib.request.Request("http://127.0.0.1:8080"+path)
    with urllib.request.urlopen(r,timeout=3) as response: return json.load(response)
def camera(): return next(s for s in get("/api/streams")["streams"] if s["id"]=="camera")
def wait(test,seconds=25):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        try:
            value=test()
            if value:return value
        except (OSError,StopIteration):pass
        time.sleep(.3)
    raise RuntimeError("bounded state transition timed out")
result={"passed":False}
try:
    result["before"]=wait(lambda: camera() if camera()["recording"]=="recording" else None)
    pid=command("systemctl","show","uvms","-p","MainPID","--value")
    command("systemctl","stop","uvms-demo-source")
    result["source_down"]=wait(lambda:camera() if not camera()["online"] else None)
    assert command("systemctl","show","uvms","-p","MainPID","--value")==pid
finally:
    command("systemctl","start","uvms-demo-source")
result["recovered"]=wait(lambda:camera() if camera()["online"] and camera()["recording"]=="recording" else None)
result["service_survived_outage"]=command("systemctl","show","uvms","-p","MainPID","--value")==pid
before_count=len(get("/api/recordings")["files"])
command("systemctl","restart","uvms")
result["after_restart"]=wait(lambda:camera() if camera()["recording"]=="recording" else None)
new_pid=command("systemctl","show","uvms","-p","MainPID","--value")
result["restart_new_pid"]=new_pid!=pid
result["finalized_file_on_restart"]=len(get("/api/recordings")["files"])>=before_count+1
result["service_children"]=command("ps","--ppid",new_pid,"-o","args=") if subprocess.run(["pgrep","-P",new_pid],stdout=subprocess.DEVNULL).returncode==0 else ""
result["ffmpeg_executable"]=subprocess.run(["nsenter","-t",new_pid,"-m","--","runuser","-u","uvms","--","test","-x","/usr/bin/ffmpeg"]).returncode==0
result["passed"]=result["service_survived_outage"] and result["restart_new_pid"] and result["finalized_file_on_restart"] and not result["service_children"] and not result["ffmpeg_executable"]
Path("evidence/port/service-lifecycle.json").write_text(json.dumps(result,indent=2))
print(json.dumps(result,indent=2))
assert result["passed"]
