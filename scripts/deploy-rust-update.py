#!/usr/bin/env python3
"""Update only the existing local Rust binary/web files, preserving its configuration and clips."""
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import time
import urllib.request

ROOT=pathlib.Path(__file__).resolve().parents[1]
binary=pathlib.Path('/root/uvms-target/release/uvms-gateway')
destination=pathlib.Path('/opt/uvms')
backup=pathlib.Path('/var/backups/uvms')/('rust-update-'+str(time.time_ns()))
digest=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
settings=[pathlib.Path('/etc/uvms/config.toml'),pathlib.Path('/etc/uvms/uvms.env')]
before_settings=[digest(p) for p in settings]
before_binary=digest(destination/'uvms-gateway')
backup.mkdir(parents=True)
shutil.copy2(destination/'uvms-gateway',backup/'uvms-gateway')
shutil.copytree(destination/'web',backup/'web')
clips={p:(p.stat().st_size,digest(p)) for p in pathlib.Path('/var/lib/uvms/recordings').rglob('*.mp4')}


def stop():
    subprocess.run(['systemctl','stop','uvms'],check=True)


def start():
    subprocess.run(['systemctl','start','uvms'],check=True)
    for _ in range(120):
        try:
            with urllib.request.urlopen('http://127.0.0.1:8080/healthz',timeout=1) as response:
                return json.load(response)
        except OSError:
            time.sleep(.5)
    raise RuntimeError('Updated Rust service did not become ready')


try:
    stop()
    # Do not rewrite service units, environment, camera URLs, or archive files.
    temporary=destination/'uvms-gateway.new'
    shutil.copy2(binary,temporary);temporary.chmod(0o755);os.replace(temporary,destination/'uvms-gateway')
    for source in (ROOT/'web').rglob('*'):
        if source.is_file():
            target=destination/'web'/source.relative_to(ROOT/'web');target.parent.mkdir(parents=True,exist_ok=True)
            temporary=target.with_name(target.name+'.new');shutil.copy2(source,temporary);temporary.chmod(0o644);os.replace(temporary,target)
    health=start()
    assert digest(destination/'uvms-gateway')==digest(binary)
    assert [digest(p) for p in settings]==before_settings
    assert all(p.exists() and (p.stat().st_size,digest(p))==entry for p,entry in clips.items())
    with urllib.request.urlopen('http://127.0.0.1:8080/app.js',timeout=5) as response:
        assert response.read()==(ROOT/'web/app.js').read_bytes()
except Exception:
    stop()
    shutil.copy2(backup/'uvms-gateway',destination/'uvms-gateway')
    shutil.copytree(backup/'web',destination/'web',dirs_exist_ok=True)
    start()
    raise

report={'passed':True,'health':health,'configuration_preserved':True,'existing_recordings_preserved':len(clips),
        'binary_before_sha256':before_binary,'binary_sha256':digest(binary),'binary_bytes':binary.stat().st_size,'backup':str(backup)}
output=ROOT/'test-results/soak/deployment.json';output.parent.mkdir(parents=True,exist_ok=True)
output.write_text(json.dumps(report,indent=2));print(json.dumps(report))
