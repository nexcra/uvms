#!/usr/bin/env python3
"""Compare startup on identical copies of the completed synthetic archive."""
import argparse
import hashlib
import json
import os
import pathlib
import signal
import sqlite3
import subprocess
import tempfile
import time
import urllib.request

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('soak_result')
parser.add_argument('--before',default='/opt/uvms/uvms-gateway')
parser.add_argument('--after',default='/root/uvms-target/release/uvms-gateway')
args=parser.parse_args()
ROOT=pathlib.Path(__file__).resolve().parents[1]
soak=json.loads(pathlib.Path(args.soak_result).read_text(encoding='utf-8-sig'))
source=pathlib.Path(soak['fixture']['recording_dir']).resolve(strict=True)
assert source.parent==pathlib.Path('/var/lib/uvms-soak')
media=list(source.glob('*/*.mp4'))
assert media and not list(source.glob('*/*.partial')), 'Stop the synthetic recorders before comparing'
original={str(p.relative_to(source)):(p.stat().st_size,hashlib.sha256(p.read_bytes()).hexdigest()) for p in media}
BASE='http://127.0.0.1:8098'


def api(route,timeout=2):
    with urllib.request.urlopen(BASE+route,timeout=timeout) as response:
        return json.load(response)


def measure(name,binary,missing=False):
    work=pathlib.Path(tempfile.mkdtemp(prefix='uvms-archive-startup-'))
    recordings=work/'recordings';recordings.mkdir()
    for p in media:
        target=recordings/p.relative_to(source);target.parent.mkdir(exist_ok=True)
        os.link(p,target)
    with sqlite3.connect(f'file:{source}/archive.sqlite?mode=ro',uri=True) as src, sqlite3.connect(recordings/'archive.sqlite') as dst:
        src.backup(dst)
        # These copies are offline startup fixtures; never start a recording writer.
        dst.execute('UPDATE controls SET json=?',(json.dumps({'mode':'off','schedules':[]}),))
        baseline=list(dst.execute('SELECT * FROM clips ORDER BY stream,file'))
        deleted=None
        if missing:
            deleted=baseline[0]
            dst.execute('DELETE FROM clips WHERE stream=? AND file=?',deleted[:2])
        dst.commit()
    config=f'''bind="127.0.0.1:8098"
rtsp_bind="127.0.0.1:18577"
ice_udp_min=51910
ice_udp_max=51910
recording_dir="{recordings}"
'''
    environment={**os.environ,'UVMS_CONFIG':str(work/'config.toml'),'PATH':'/nonexistent'}
    for alias in soak['fixture']['aliases']:
        config+=f'[streams.{alias}]\nurl_env="ARCHIVE_{alias.upper()}"\nrecord=false\n'
        environment['ARCHIVE_'+alias.upper()]='rtsp://127.0.0.1:18576/'+alias
    (work/'config.toml').write_text(config)
    process=None
    try:
        start=time.monotonic()
        with (work/'server.log').open('wb') as log:
            process=subprocess.Popen([binary],env=environment,stdout=log,stderr=log)
        while time.monotonic()-start<240:
            assert process.poll() is None, (work/'server.log').read_text()
            try:
                health=api('/healthz',.3)
                break
            except OSError:
                time.sleep(.05)
        else:
            raise AssertionError('Archive startup exceeded 240 seconds')
        elapsed=time.monotonic()-start
        listing=api('/api/recordings?limit=1000')
        assert len(listing['files'])==len(media)
        assert listing['used_bytes']==sum(v[0] for v in original.values()), 'Skipped files escaped quota accounting'
        with sqlite3.connect(recordings/'archive.sqlite') as db:
            after=list(db.execute('SELECT * FROM clips ORDER BY stream,file'))
        if missing:
            assert all(row in after for row in baseline if row!=deleted)
            imported=next(row for row in after if row[:2]==deleted[:2])
            assert imported[5]==0 and imported[3]>imported[2] and imported[4]==deleted[4], 'Unindexed clip was not imported'
        else:
            assert baseline==after, 'Exact index metadata changed'
        return {'case':name,'seconds_to_http_ready':elapsed,'files':len(media),'used_bytes':listing['used_bytes'],
                'version':health['version'],'index_preserved':True,'unindexed_import_checked':missing,
                'binary_sha256':hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest(),'work':str(work)}
    finally:
        if process is not None:
            process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill();process.wait()
                raise AssertionError('Startup fixture failed to stop')


results=[]
for name,binary,missing in [('before',args.before,False),('after',args.after,False),('after-unindexed',args.after,True)]:
    result=measure(name,binary,missing);results.append(result);print(json.dumps(result),flush=True)
assert all((p.stat().st_size,hashlib.sha256(p.read_bytes()).hexdigest())==original[str(p.relative_to(source))] for p in media)
report={'passed':True,'recordings_unchanged':True,'results':results}
output=ROOT/'test-results/soak/archive-startup.json';output.write_text(json.dumps(report,indent=2))
