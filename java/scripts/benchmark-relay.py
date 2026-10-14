"""Sixteen bounded native RTSP clients, no gateway credentials or saved media."""
import concurrent.futures, json, os, pathlib, re, subprocess, time, urllib.parse
root = pathlib.Path(__file__).resolve().parents[1]
service = os.environ.get('UVMS_BENCH_SERVICE', 'uvms-java')
relay_url = os.environ.get('UVMS_BENCH_RTSP', 'rtsp://127.0.0.1:8558/camera_live')
pid = int(subprocess.check_output(['systemctl','show','-p','MainPID','--value',service], text=True))
hz = os.sysconf('SC_CLK_TCK')
def resources():
    p = pathlib.Path(f'/proc/{pid}')
    fields = (p/'stat').read_text().rsplit(')',1)[1].split()
    rss = next(l for l in (p/'status').read_text().splitlines() if l.startswith('VmRSS:'))
    return (int(fields[11])+int(fields[12]))/hz, int(rss.split()[1])/1024
def viewer(_):
    env = {**os.environ, 'REAL_CAMERA_RTSP_URL':relay_url}
    env.pop('RTSP_CAPTURE_DIR',None)
    start = time.monotonic()
    process = subprocess.run(['java', '-cp', str(root/'target/uvms-java-0.1.0.jar' if (root/'target/uvms-java-0.1.0.jar').is_file() else root/'uvms-java.jar'), 'uvms.Probe'],env=env,capture_output=True,text=True,timeout=40)
    frames = re.findall(r'frame=\d+ timestamp=\d+ key=(?:true|false) bytes=(\d+)',process.stdout)
    return {'passed':process.returncode==0 and len(frames)==30,'elapsed_seconds':time.monotonic()-start,
            'frames':len(frames),'bytes':sum(map(int, frames))}
cpu_before,rss=resources(); start=time.monotonic(); peak=rss
with concurrent.futures.ThreadPoolExecutor(max_workers=16) as executor:
    futures=[executor.submit(viewer,i) for i in range(16)]
    while not all(f.done() for f in futures):
        peak=max(peak,resources()[1]); time.sleep(.25)
    clients=[f.result() for f in futures]
elapsed=time.monotonic()-start;cpu_after,_=resources()
report={'protocol':'rtsp-tcp','service':service,'viewers':16,'camera':urllib.parse.urlsplit(relay_url).path.strip('/'),'credentials_required':False,
        'passed':all(c['passed'] for c in clients),'clients':clients,'measurement_includes_startup':True,
        'elapsed_seconds':elapsed,'cpuMeanOneCorePercent':100*(cpu_after-cpu_before)/elapsed,'rssPeakMiB':peak,
        'payloadMbps':8*sum(c['bytes'] for c in clients)/elapsed/1e6,'frames':sum(c['frames'] for c in clients)}
out=root/os.environ.get('UVMS_BENCH_OUTPUT','test-results/performance')/'relay.json';out.parent.mkdir(parents=True,exist_ok=True)
out.write_text(json.dumps(report,indent=2));print(json.dumps(report))
if not report['passed']:raise SystemExit(1)
