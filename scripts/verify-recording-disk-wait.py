#!/usr/bin/env python3
"""Compare recorders while four first segment fsync calls each stall for five seconds."""
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

ROOT=pathlib.Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--before',required=True,help='Existing baseline Linux executable without the recording backlog fix')
parser.add_argument('--after',default='/root/uvms-target/release/uvms-gateway')
args=parser.parse_args()
WORK=pathlib.Path(tempfile.mkdtemp(prefix='uvms-recording-disk-wait-'))
ALIASES=['aac','pcmu','pcma','opus']
(WORK/'delay.c').write_text(r'''
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
static atomic_int count;
int fsync(int fd) {
    char link[80], path[4096];
    snprintf(link,sizeof link,"/proc/self/fd/%d",fd);
    ssize_t n=readlink(link,path,sizeof(path)-1);
    if(n>0){path[n]=0;if(strstr(path,".partial") && atomic_fetch_add(&count,1)<4){
        struct timespec delay={5,0};
        while(nanosleep(&delay,&delay)<0){}
        fprintf(stderr,"injected recording fsync delay: 5 seconds\n");
    }}
    return ((int(*)(int))dlsym(RTLD_NEXT,"fsync"))(fd);
}
''')
subprocess.run(['cc','-shared','-fPIC','-O2',str(WORK/'delay.c'),'-ldl','-o',str(WORK/'delay.so')],check=True)


def request(route,body=None):
    req=urllib.request.Request('http://127.0.0.1:8098'+route,
        data=json.dumps(body).encode() if body is not None else None,
        headers={'Content-Type':'application/json'})
    with urllib.request.urlopen(req,timeout=5) as response:return json.load(response)


def run(label,binary):
    directory=WORK/label;directory.mkdir()
    config=f'''bind="127.0.0.1:8098"
rtsp_bind="127.0.0.1:18577"
ice_udp_min=51910
ice_udp_max=51910
recording_dir="{directory}/recordings"
segment_seconds=15
recording_limit_bytes=536870912
'''
    environment={**os.environ,'PATH':'/nonexistent','UVMS_CONFIG':str(directory/'config.toml'),'LD_PRELOAD':str(WORK/'delay.so')}
    for alias in ALIASES:
        config+=f'[streams.{alias}]\nurl_env="WAIT_{alias.upper()}"\nrecord=true\n'
        environment['WAIT_'+alias.upper()]='rtsp://127.0.0.1:18576/'+alias
    (directory/'config.toml').write_text(config)
    with (directory/'service.log').open('wb') as log:
        process=subprocess.Popen([binary],env=environment,stdout=log,stderr=log)
    try:
        deadline=time.monotonic()+30
        while True:
            try:
                if request('/healthz')['status']=='ok':break
            except OSError:pass
            if time.monotonic()>deadline:raise AssertionError('service startup timeout')
            time.sleep(.1)
        for _ in range(50):
            assert process.poll() is None
            time.sleep(1)
        for alias in ALIASES:request('/api/recording/'+alias,{'mode':'off','schedules':[]})
        time.sleep(2)
    finally:
        process.send_signal(signal.SIGTERM)
        try:process.wait(timeout=15)
        except subprocess.TimeoutExpired:process.kill();process.wait();raise
    assert process.returncode==0
    log=(directory/'service.log').read_text()
    assert log.count('injected recording fsync delay')==4
    db=sqlite3.connect(directory/'recordings/archive.sqlite');db.row_factory=sqlite3.Row
    clips=[dict(row) for row in db.execute('SELECT * FROM clips ORDER BY stream,start_ms')];db.close()
    gaps=[];decode_errors=[]
    for alias in ALIASES:
        ordered=[c for c in clips if c['stream']==alias]
        assert len(ordered)>=2
        for previous,current in zip(ordered,ordered[1:]):
            gap=current['start_ms']-previous['end_ms']
            if gap>2000:gaps.append({'camera':alias,'milliseconds':gap})
    for clip in clips:
        probe=subprocess.run(['ffprobe','-v','error','-count_frames','-show_entries','stream=codec_type,nb_read_frames','-of','json',str(directory/'recordings'/clip['stream']/clip['file'])],capture_output=True,text=True,timeout=20)
        tracks=json.loads(probe.stdout).get('streams',[])
        if probe.returncode or probe.stderr.strip() or sorted(t['codec_type'] for t in tracks)!=['audio','video'] or any(int(t.get('nb_read_frames',0))==0 for t in tracks):
            decode_errors.append(clip['stream']+'/'+clip['file'])
    return {'case':label,'binary_sha256':hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest(),
            'recordings':len(clips),'injected_fsync_wait_seconds':5,'delayed_calls':4,
            'unexpected_gaps':gaps,'decode_errors':decode_errors,
            'recording_interruptions':log.count('Recording interrupted'),'work':str(directory)}


before=run('before',str(pathlib.Path(args.before).resolve(strict=True)));print(json.dumps(before),flush=True)
after=run('after',str(pathlib.Path(args.after).resolve(strict=True)));print(json.dumps(after),flush=True)
report={'passed':bool(before['unexpected_gaps'] or before['recording_interruptions']) and not(after['unexpected_gaps'] or after['decode_errors'] or after['recording_interruptions']), 'results':[before,after]}
(ROOT/'test-results/soak/disk-wait.json').write_text(json.dumps(report,indent=2))
raise SystemExit(0 if report['passed'] else 1)
