#!/usr/bin/env python3
"""Apply the verified build to this local WSL service, preserving live inputs."""
import hashlib, json, os, pathlib, subprocess, time, urllib.request

root = pathlib.Path(__file__).resolve().parents[1]
output = root / 'test-results/rust-optimization/deployment.json'
binary = pathlib.Path('/root/uvms-target/release/uvms-gateway')
deployed = pathlib.Path('/opt/uvms/uvms-gateway')
backup = root / 'test-results/rust-optimization/before/uvms-gateway'
paths = [pathlib.Path('/etc/uvms/config.toml'), pathlib.Path('/etc/uvms/uvms.env')]
digest = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
before = [digest(p) for p in paths]
old_binary = digest(deployed)
assert digest(backup) == old_binary, 'Baseline backup does not match deployed binary'
java_path = pathlib.Path('/opt/uvms-java/uvms-java.jar')
java_before = digest(java_path)
existing = {p: (p.stat().st_size, digest(p)) for p in pathlib.Path('/var/lib/uvms/recordings').rglob('*.mp4')}

def ready():
    for attempt in range(40):
        try:
            with urllib.request.urlopen('http://127.0.0.1:8080/healthz', timeout=1) as response:
                assert response.status == 200
                return json.load(response)
        except OSError:
            time.sleep(.25)
    raise RuntimeError('Deployed service did not become healthy')

try:
    subprocess.run(['systemctl', 'stop', 'uvms'], check=True)
    subprocess.run(['bash', 'scripts/install-linux.sh'], cwd=root,
                   env={**os.environ, 'UVMS_BIN': str(binary)}, check=True)
    assert [digest(p) for p in paths] == before, 'Input configuration changed'
    subprocess.run(['systemctl', 'start', 'uvms'], check=True)
    health = ready()
except Exception:
    subprocess.run(['systemctl', 'stop', 'uvms'], check=False)
    subprocess.run(['install', '-m', '755', str(backup), str(deployed)], check=True)
    subprocess.run(['systemctl', 'start', 'uvms'], check=True)
    raise

assert digest(deployed) == digest(binary)
assert all(p.exists() and (p.stat().st_size, digest(p)) == info for p, info in existing.items()), 'Existing recording changed'
pid = subprocess.check_output(['systemctl', 'show', 'uvms', '-p', 'MainPID', '--value'], text=True).strip()
children = pathlib.Path(f'/proc/{pid}/task/{pid}/children').read_text().split()
ffmpeg_executable = subprocess.run(['nsenter', '-t', pid, '-m', '--', 'runuser', '-u', 'uvms', '--', 'test', '-x', '/usr/bin/ffmpeg']).returncode == 0
java_unchanged = digest(java_path) == java_before
java_active = subprocess.run(['systemctl', 'is-active', '--quiet', 'uvms-java']).returncode == 0
report = {'passed': not children and not ffmpeg_executable and java_unchanged and java_active,
          'service': 'uvms', 'health': health, 'binary_bytes': deployed.stat().st_size,
          'binary_before_sha256': old_binary, 'binary_sha256': digest(deployed),
          'configuration_preserved': True, 'existing_recordings_preserved': len(existing),
          'camera_credentials_mode': oct(paths[1].stat().st_mode & 0o777),
          'child_processes': children, 'ffmpeg_executable': ffmpeg_executable,
          'java_unchanged': java_unchanged, 'java_active': java_active}
output.write_text(json.dumps(report, indent=2))
print(json.dumps(report))
assert report['passed']
