#!/usr/bin/env python3
"""Upgrade the local WSL Rust service; preserve inputs, recordings and Java."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import urllib.request

root = Path(__file__).resolve().parents[1]
output = root / os.environ.get('UVMS_DEPLOY_EVIDENCE', 'test-results/surveillance/deployment.json')
binary = Path('/root/uvms-target/release/uvms-gateway')
deployed = Path('/opt/uvms/uvms-gateway')
web = Path('/opt/uvms/web')
unit = Path('/etc/systemd/system/uvms.service')
backup = Path('/var/backups/uvms') / ('surveillance-' + str(time.time_ns()))
backup.mkdir(parents=True)
digest = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
paths = [Path('/etc/uvms/config.toml'), Path('/etc/uvms/uvms.env')]
before = [digest(p) for p in paths]
old_binary = digest(deployed)
shutil.copy2(deployed, backup / 'uvms-gateway')
shutil.copytree(web, backup / 'web')
shutil.copy2(unit, backup / 'uvms.service')
java = Path('/opt/uvms-java/uvms-java.jar')
java_before = digest(java)
existing = {p: (p.stat().st_size, digest(p)) for p in Path('/var/lib/uvms/recordings').rglob('*.mp4')}

def ready():
    for _ in range(120):
        try:
            with urllib.request.urlopen('http://127.0.0.1:8080/healthz', timeout=1) as r:
                assert r.status == 200
                return json.load(r)
        except OSError:
            time.sleep(.5)
    raise RuntimeError('Updated service did not become healthy')

try:
    subprocess.run(['systemctl', 'stop', 'uvms'], check=True)
    subprocess.run(['bash', 'scripts/install-linux.sh'], cwd=root,
                   env={**os.environ, 'UVMS_BIN': str(binary)}, check=True)
    assert [digest(p) for p in paths] == before, 'Input configuration changed'
    subprocess.run(['systemctl', 'start', 'uvms'], check=True)
    health = ready()
    assert digest(deployed) == digest(binary)
    assert all(p.exists() and (p.stat().st_size, digest(p)) == info for p, info in existing.items()), 'Existing recording changed'
    assert digest(java) == java_before, 'Java binary changed'
    assert subprocess.run(['systemctl', 'is-active', '--quiet', 'uvms-java']).returncode == 0
    for path in ['/api/streams', '/api/recordings?limit=1', '/vendor/hls.min.js']:
        with urllib.request.urlopen('http://127.0.0.1:8080' + path, timeout=10) as r:
            assert r.status == 200
    pid = subprocess.check_output(['systemctl', 'show', 'uvms', '-p', 'MainPID', '--value'], text=True).strip()
    children = Path(f'/proc/{pid}/task/{pid}/children').read_text().split()
    ffmpeg_executable = subprocess.run(['nsenter', '-t', pid, '-m', '--', 'runuser', '-u', 'uvms', '--', 'test', '-x', '/usr/bin/ffmpeg']).returncode == 0
    assert not children and not ffmpeg_executable
except Exception:
    subprocess.run(['systemctl', 'stop', 'uvms'], check=False)
    shutil.copy2(backup / 'uvms-gateway', deployed)
    shutil.copytree(backup / 'web', web, dirs_exist_ok=True)
    shutil.copy2(backup / 'uvms.service', unit)
    subprocess.run(['systemctl', 'daemon-reload'], check=True)
    subprocess.run(['systemctl', 'start', 'uvms'], check=True)
    raise

report = {'passed': True, 'service': 'uvms', 'health': health,
          'binary_bytes': deployed.stat().st_size, 'binary_before_sha256': old_binary,
          'binary_sha256': digest(deployed), 'backup': str(backup),
          'configuration_preserved': True, 'existing_recordings_preserved': len(existing),
          'child_processes': children, 'ffmpeg_executable': ffmpeg_executable,
          'java_unchanged': True, 'java_active': True}
output.parent.mkdir(parents=True, exist_ok=True)
output.write_text(json.dumps(report, indent=2))
print(json.dumps(report))
