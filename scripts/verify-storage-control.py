#!/usr/bin/env python3
"""Verify live quota updates, recording intent and persistence on isolated sources."""
import hashlib
import json
import pathlib
import subprocess
import time
import urllib.error
import urllib.request

ROOT=pathlib.Path(__file__).resolve().parents[1]
BASE='http://127.0.0.1:8099'
ALIASES=['aac','pcmu','pcma','opus']
report={'passed':False}


def fixture(action,*args):
    subprocess.run(['python3',str(ROOT/'scripts/soak-fixture.py'),action,*args],check=True,capture_output=True)


def api(path,data=None,method=None):
    request=urllib.request.Request(BASE+path,data=None if data is None else json.dumps(data).encode(),method=method,headers={'Content-Type':'application/json'})
    try:
        response=urllib.request.urlopen(request,timeout=8)
    except urllib.error.HTTPError as error:
        response=error
    with response:
        content=response.read()
        try:
            value=json.loads(content)
        except ValueError:
            value=content.decode()
        return response.status,value


def wait(predicate,seconds=35):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        try:
            if result:=predicate():return result
        except (OSError,ValueError):pass
        time.sleep(.2)
    raise AssertionError('Storage-control condition timed out')


def states():
    return {c['id']:c['recording'] for c in api('/api/status')[1]['cameras']}


def control(alias,mode,schedules=None):
    assert api('/api/recording/'+alias,{'mode':mode,'schedules':schedules or []})[0]==200


def quota(value):
    code,body=api('/api/storage',{'limit_bytes':value},'PUT')
    assert code==200 and body['limit_bytes']==value
    return body


def pid():
    return subprocess.check_output(['systemctl','show','uvms-soak','--property=MainPID','--value'],text=True).strip()


def files(root):
    return {str(p.relative_to(root)):{'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in root.glob('*/*.mp4')}


started=time.monotonic()
try:
    fixture('setup','--binary','/root/uvms-target/release/uvms-gateway')
    identity=json.loads(pathlib.Path('/etc/uvms-soak/run.json').read_text())
    root=pathlib.Path(identity['recording_dir'])
    report['fixture']=identity
    initial_pid=pid()
    time.sleep(2)
    quota(4*1024*1024)
    for alias in ALIASES:control(alias,'on')
    wait(lambda:all(s=='storage_full' for s in states().values()))
    original=files(root)
    assert len(original)==4 and api('/readyz')[0]==503
    used=api('/api/storage')[1]['used_bytes']
    assert used==sum(v['bytes'] for v in original.values()) and used<=4*1024*1024
    before=time.monotonic()
    quota(16*1024*1024)
    wait(lambda:all(s=='recording' for s in states().values()))
    wait(lambda:api('/readyz')[0]==200)
    report['resume_seconds']=round(time.monotonic()-before,3)
    assert pid()==initial_pid
    time.sleep(5)
    # A decrease cannot remove existing data; it stops future reservations.
    control('pcma','off')
    now=int(time.time()*1000)
    control('opus','auto',[{'start_ms':now-1000,'end_ms':now+5000}])
    quota(1048576)
    wait(lambda:states()['aac']==states()['pcmu']==states()['opus']=='storage_full' and states()['pcma']=='off')
    before_files=files(root)
    assert api('/api/storage')[1]['used_bytes']>1048576
    wait(lambda:states()['opus']=='scheduled')
    assert files(root)==before_files
    report['schedule_expires_while_storage_full']=True
    quota(32*1024*1024)
    wait(lambda:states()=={'aac':'recording','pcmu':'recording','pcma':'off','opus':'scheduled'})
    assert pid()==initial_pid
    report['manual_off_and_expired_schedule_preserved']=True
    report['same_process_during_limit_changes']=True
    # A recording state begins on its first IDR. Let both media tracks arrive
    # before deliberately ending the session; a 1-frame clip may have no audio.
    time.sleep(3)
    fixture('restart-gateway')
    wait(lambda:states()=={'aac':'recording','pcmu':'recording','pcma':'off','opus':'scheduled'})
    assert api('/api/storage')[1]['limit_bytes']==32*1024*1024
    report['limit_and_recording_intent_persisted']=True
    time.sleep(3)
    for alias in ALIASES:control(alias,'off')
    wait(lambda:all(s=='off' for s in states().values()))
    final=files(root)
    assert all(final[name]==value for name,value in original.items())
    assert api('/api/storage')[1]['used_bytes']==sum(v['bytes'] for v in final.values())
    for data in [{'limit_bytes':0},{'limit_bytes':2**64-1},{},{'limit_bytes':1048576,'delete':True}]:
        assert api('/api/storage',data,'PUT')[0] in [400,422]
    assert api('/api/storage')[1]['limit_bytes']==32*1024*1024
    report['existing_recordings_preserved']=len(original)
    report['completed_recordings']=len(final)
    report['final_storage']=api('/api/storage')[1]
    probes=[]
    for filename in final:
        process=subprocess.run(['ffprobe','-v','error','-count_frames','-show_entries','stream=codec_type,codec_name,nb_read_frames','-of','json',str(root/filename)],capture_output=True,text=True,timeout=15)
        result=json.loads(process.stdout)
        assert process.returncode==0 and not process.stderr.strip(),process.stderr
        assert sorted(s['codec_type'] for s in result['streams'])==['audio','video']
        assert all(int(s.get('nb_read_frames',0))>0 for s in result['streams']),filename
        probes.append({'file':filename,'streams':result['streams']})
    report['probes']=probes
    events=api('/api/events?limit=1000')[1]['events']
    report['storage_events']=[e for e in events if e['kind']=='storage']
    assert len(report['storage_events'])==4
    report['passed']=True
except Exception as error:
    report['error']=repr(error)
    raise
finally:
    report['elapsed_seconds']=round(time.monotonic()-started,3)
    output=ROOT/'test-results/storage-control/result.json'
    output.parent.mkdir(parents=True,exist_ok=True)
    output.write_text(json.dumps(report,indent=2))
    fixture('stop')
    print(json.dumps({k:report[k] for k in ['passed','elapsed_seconds','error'] if k in report}))
