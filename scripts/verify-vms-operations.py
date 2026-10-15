#!/usr/bin/env python3
"""Exercise engine operations with four synthetic cameras; owns uvms-soak* only."""
import hashlib
import json
import pathlib
import subprocess
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASE = 'http://127.0.0.1:8099'
ALIASES = ['aac', 'pcmu', 'pcma', 'opus']
report = {'passed': False, 'cameras': ALIASES}


def fixture(action, *args):
    subprocess.run(['/usr/bin/python3', str(ROOT/'scripts/soak-fixture.py'), action, *args], check=True, capture_output=True)


def api(path, data=None):
    request = urllib.request.Request(BASE+path, data=None if data is None else json.dumps(data).encode(),
                                     headers={'Content-Type': 'application/json'})
    try:
        response = urllib.request.urlopen(request, timeout=8)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        return response.status, json.load(response)


def wait(predicate, seconds=45):
    deadline = time.monotonic()+seconds
    while time.monotonic() < deadline:
        try:
            if result := predicate():
                return result
        except (OSError, ValueError):
            pass
        time.sleep(.25)
    raise AssertionError('Isolated VMS condition timed out')


def recording(alias, mode):
    code, _ = api('/api/recording/'+alias, {'mode': mode, 'schedules': []})
    assert code == 200


def all_recording(state):
    code, value = api('/api/status')
    return code == 200 and all(c['recording'] == state for c in value['cameras'])


def journal(after=0, stream=None):
    code, value = api('/api/events?limit=1000&after_id='+str(after)+('' if stream is None else '&stream='+stream))
    assert code == 200
    return value['events']


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


started = time.monotonic()
try:
    fixture('setup', '--binary', '/root/uvms-target/release/uvms-gateway')
    identity = json.loads(pathlib.Path('/etc/uvms-soak/run.json').read_text())
    root = pathlib.Path(identity['recording_dir'])
    report['fixture'] = identity
    start_ms = int(time.time()*1000)
    code, state = api('/readyz')
    assert code == 200 and all(not c['input_active'] for c in state['cameras'])
    report['idle_ready_without_camera_connections'] = True
    for alias in ALIASES:
        recording(alias, 'on')
    wait(lambda: all_recording('recording'))
    wait(lambda: api('/readyz')[0] == 200)
    time.sleep(18)
    cursor = journal()[-1]['id']
    fixture('source-off', '--camera', 'aac')
    wait(lambda: any(e['kind'] == 'input' and e['state'] == 'interrupted' for e in journal(cursor, 'aac')))
    wait(lambda: api('/readyz')[0] == 503)
    degraded = api('/api/status')[1]
    assert all(c['recording'] == 'recording' for c in degraded['cameras'] if c['id'] != 'aac')
    time.sleep(5)
    restored_at = time.monotonic()
    fixture('source-on', '--camera', 'aac')
    wait(lambda: any(e['kind'] == 'input' and e['state'] == 'online' for e in journal(cursor, 'aac')))
    wait(lambda: all_recording('recording') and api('/readyz')[0] == 200)
    report['recovery_seconds'] = round(time.monotonic()-restored_at, 3)
    report['outage_state'] = degraded
    time.sleep(18)

    # Force termination on test instance only. Existing completed recordings and
    # unfinished fragments must survive, without exposing partials as good clips.
    completed_before = {str(p): digest(p) for p in root.glob('*/*.mp4')}
    assert len(completed_before) >= 4
    subprocess.run(['systemctl', 'kill', '--kill-whom=main', '--signal=SIGKILL', 'uvms-soak'], check=True)
    wait(lambda: subprocess.run(['systemctl', 'is-active', '--quiet', 'uvms-soak']).returncode != 0)
    partials = {str(p): digest(p) for p in root.glob('*/*.partial')}
    assert partials
    subprocess.run(['systemctl', 'start', 'uvms-soak'], check=True)
    wait(lambda: all_recording('recording'))
    wait(lambda: any(e['state'] == 'previous_shutdown_unconfirmed' for e in journal()))
    assert all(digest(pathlib.Path(p)) == sha for p, sha in completed_before.items())
    assert all(digest(pathlib.Path(p)) == sha for p, sha in partials.items())
    state = api('/api/status')[1]
    assert state['storage']['unfinished_files_at_startup'] == len(partials)
    report['crash_completed_files_preserved'] = len(completed_before)
    report['crash_partial_files_preserved'] = len(partials)
    time.sleep(18)
    for alias in ALIASES:
        recording(alias, 'off')
    wait(lambda: all_recording('off'))
    end_ms = int(time.time()*1000)
    timelines = {}
    for alias in ALIASES:
        route = f'/api/timeline/{alias}?from={start_ms}&to={end_ms}'
        code, value = api(route)
        assert code == 200
        assert value['completed_clips'] == len(list((root/alias).glob('*.mp4')))
        assert value['unavailable_clips'] == value['unknown_duration_clips'] == 0
        assert value['recorded_ms'] > 20000 and value['gap_ms'] > 0
        assert value['recorded_ms'] + value['gap_ms'] == end_ms-start_ms
        timelines[alias] = value
    report['timelines'] = timelines
    candidate = next((root/'aac').glob('*.mp4'))
    hidden = candidate.with_suffix('.offline')
    candidate.rename(hidden)
    try:
        value = api(f'/api/timeline/aac?from={start_ms}&to={end_ms}')[1]
        assert value['unavailable_clips'] == 1
        assert value['recorded_ms'] < timelines['aac']['recorded_ms']
    finally:
        hidden.rename(candidate)
    report['missing_file_excluded_from_coverage'] = True
    for route, expected in [('/api/timeline/aac?from=20&to=10',400),('/api/timeline/nope?from=1&to=2',404),('/api/events?after_id=18446744073709551615',400)]:
        # Error responses can be empty/plain text, so inspect status only.
        try:
            with urllib.request.urlopen(BASE+route, timeout=5) as response:
                code=response.status
        except urllib.error.HTTPError as error:
            code=error.code
        assert code == expected
    pages=[]
    cursor=0
    while True:
        code, page=api('/api/events?limit=7&after_id='+str(cursor))
        assert code == 200
        if not page['events']:
            break
        assert page['next_after_id'] > cursor
        cursor=page['next_after_id'];pages.extend(page['events'])
    assert len({e['id'] for e in pages}) == len(pages)
    assert all(e['stream'] == 'aac' for e in journal(stream='aac'))
    report['events'] = pages
    fixture('restart-gateway')
    wait(lambda: all_recording('off'))
    assert api('/readyz')[0] == 200
    for alias in ALIASES:
        assert api('/api/recording/'+alias)[1]['mode'] == 'off'
    assert any(e['kind'] == 'service' and e['state'] == 'stopped' for e in journal())
    assert set(e['id'] for e in pages).issubset({e['id'] for e in journal()})
    report['graceful_restart_preserves_journal_and_controls'] = True
    probes=[]
    for path in root.glob('*/*.mp4'):
        result=subprocess.run(['/usr/bin/ffprobe','-v','error','-count_frames','-show_entries','stream=codec_type,codec_name,nb_read_frames','-of','json',str(path)],capture_output=True,text=True,timeout=20)
        value=json.loads(result.stdout)
        assert result.returncode == 0 and not result.stderr.strip(), result.stderr
        assert sorted(s['codec_type'] for s in value['streams']) == ['audio','video']
        assert all(int(s['nb_read_frames']) > 0 for s in value['streams'])
        probes.append({'file':str(path.relative_to(root)),'streams':value['streams']})
    report['decoded_recordings'] = probes
    report['final_status'] = api('/api/status')[1]
    assert report['final_status']['journal']['lost_events'] == 0
    report['passed'] = True
except Exception as error:
    report['error'] = repr(error)
    raise
finally:
    report['elapsed_seconds'] = round(time.monotonic()-started,3)
    output=ROOT/'test-results/vms-operations/result.json'
    output.parent.mkdir(parents=True,exist_ok=True)
    output.write_text(json.dumps(report,indent=2))
    fixture('stop')
    print(json.dumps({key:report[key] for key in ['passed','elapsed_seconds','error'] if key in report}))
