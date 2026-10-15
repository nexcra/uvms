#!/usr/bin/env python3
"""Four concurrent recorders reaching one shared quota; isolated synthetic inputs only."""
import hashlib
import json
import os
import pathlib
import signal
import subprocess
import tempfile
import time
import urllib.request
import urllib.error

ROOT = pathlib.Path(__file__).resolve().parents[1]
WORK = pathlib.Path(tempfile.mkdtemp(prefix='uvms-soak-quota-'))
BASE = 'http://127.0.0.1:8098'
LIMIT = 1048576
ALIASES = ['aac','pcmu','pcma','opus']
config = f'''bind="127.0.0.1:8098"
rtsp_bind="127.0.0.1:18577"
ice_udp_min=51910
ice_udp_max=51910
recording_dir="{WORK}/recordings"
segment_seconds=2
recording_limit_bytes={LIMIT}
'''
environment = {**os.environ, 'PATH':'/nonexistent', 'UVMS_CONFIG':str(WORK/'config.toml')}
for alias in ALIASES:
    config += f'[streams.{alias}]\nurl_env="QUOTA_{alias.upper()}"\nrecord=true\n'
    environment['QUOTA_'+alias.upper()]='rtsp://127.0.0.1:18576/'+alias
(WORK/'config.toml').write_text(config)


def api(route):
    with urllib.request.urlopen(BASE+route, timeout=3) as response:
        return json.load(response)


def wait(predicate, timeout=30):
    deadline = time.monotonic()+timeout
    while time.monotonic()<deadline:
        try:
            if value := predicate():
                return value
        except OSError:
            pass
        time.sleep(.2)
    raise AssertionError('Timed out waiting for isolated quota condition')


def files():
    return {str(p.relative_to(WORK/'recordings')):{'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()}
            for p in (WORK/'recordings').glob('*/*') if p.suffix in ['.mp4','.partial']}


def stop(process):
    if process.poll() is None:
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=12)
        except subprocess.TimeoutExpired:
            process.kill();process.wait()
            raise AssertionError('Graceful quota shutdown timed out')
    assert process.returncode==0, process.returncode


report={'work':str(WORK),'quota_bytes':LIMIT,'passed':False}
process=None
try:
    for run in range(2):
        with (WORK/f'run-{run}.log').open('wb') as log:
            process=subprocess.Popen(['/opt/uvms-soak/uvms-gateway'],env=environment,stdout=log,stderr=log)
        wait(lambda: process.poll() is None and api('/healthz')['status']=='ok')
        wait(lambda: all(s['recording']=='storage_full' for s in api('/api/streams')['streams']))
        before=files()
        assert before and all(n.endswith('.mp4') for n in before), before
        assert set(n.split('/')[0] for n in before)==set(ALIASES), 'Some cameras never recorded'
        assert sum(f['bytes'] for f in before.values())<=LIMIT
        for _ in range(6):
            time.sleep(1)
            assert files()==before, 'Recording changed after quota reached'
        listing=api('/api/recordings')
        assert listing['used_bytes']==sum(f['bytes'] for f in before.values())
        assert len(listing['files'])==len(before)
        try:
            urllib.request.urlopen(BASE+'/readyz',timeout=3)
            raise AssertionError('Full recording quota must report degraded readiness')
        except urllib.error.HTTPError as error:
            assert error.code==503
            readiness=json.load(error)
        assert readiness['status']=='degraded'
        assert all(c['recording_requested'] and not c['recording_ok'] for c in readiness['cameras'])
        events=api('/api/events?limit=1000')['events']
        assert {e['stream'] for e in events if e['kind']=='recording' and e['state']=='storage_full'}==set(ALIASES)
        report.setdefault('readiness',[]).append(readiness)
        stop(process);process=None
        if run==0:
            report['files']=before
        else:
            assert before==report['files'], 'Restart changed existing recordings'
    probes=[]
    for name in report['files']:
        p=subprocess.run(['/usr/bin/ffprobe','-v','error','-count_frames','-show_entries','stream=codec_type,codec_name,nb_read_frames','-of','json',str(WORK/'recordings'/name)],capture_output=True,text=True,timeout=15)
        data=json.loads(p.stdout)
        assert p.returncode==0 and not p.stderr.strip(), p.stderr
        assert sorted(t['codec_type'] for t in data['streams'])==['audio','video']
        assert all(int(t.get('nb_read_frames',0))>0 for t in data['streams'])
        probes.append({'file':name,'streams':data['streams']})
    report.update(passed=True,probes=probes,restarts=1,stable_seconds_per_run=6)
except Exception as error:
    report['error']=str(error)
finally:
    if process is not None:
        stop(process)
    output=ROOT/os.environ.get('UVMS_QUOTA_OUTPUT','test-results/soak/quota.json');output.parent.mkdir(parents=True,exist_ok=True)
    output.write_text(json.dumps(report,indent=2));print(json.dumps(report))
if not report['passed']:
    raise SystemExit(1)
