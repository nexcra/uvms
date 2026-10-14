#!/usr/bin/env python3
"""Check native release identity and excluded private/vendor artifacts."""
import hashlib, json, os, pathlib, shlex, tarfile, urllib.parse, zipfile

root = pathlib.Path(__file__).resolve().parents[1]
manifest = json.loads((root / 'dist/native-manifest.json').read_text())
secrets = []
for line in pathlib.Path('/etc/uvms/uvms.env').read_text().splitlines():
    for word in shlex.split(line, comments=True):
        if '=' in word:
            value = word.split('=', 1)[1]
            if value.startswith('rtsp://'):
                password = urllib.parse.urlsplit(value).password
                if password:
                    secrets.extend([password.encode(), urllib.parse.unquote(password).encode()])
checked = 0
binary_seen = False

def inspect(name, data):
    global checked, binary_seen
    checked += 1
    assert not any(secret in data for secret in secrets), 'Credential content detected'
    assert not name.lower().endswith(('.msi', '.dll', '.pem', '.mp4', '.h264')), 'Excluded artifact detected'
    assert not name.endswith('/uvms.env'), 'Live environment file detected'
    if name.endswith('/uvms-gateway'):
        assert hashlib.sha256(data).hexdigest() == manifest['binary_sha256']
        binary_seen = True

for artifact in manifest['artifacts']:
    path = root / 'dist' / artifact['name']
    assert path.stat().st_size == artifact['bytes']
    assert hashlib.sha256(path.read_bytes()).hexdigest() == artifact['sha256']
    if path.suffix == '.zip':
        with zipfile.ZipFile(path) as archive:
            for entry in archive.infolist():
                if not entry.is_dir():
                    inspect(entry.filename, archive.read(entry))
    else:
        with tarfile.open(path) as archive:
            for entry in archive:
                if entry.isfile():
                    inspect(entry.name, archive.extractfile(entry).read())
assert binary_seen
assert hashlib.sha256(pathlib.Path('/opt/uvms/uvms-gateway').read_bytes()).hexdigest() == manifest['binary_sha256']
result = {'passed': True, 'files_inspected': checked, 'deployed_binary_matches': True,
          'camera_credentials_absent': True, 'vendor_binaries_absent': True,
          'artifacts': manifest['artifacts']}
output = root / os.environ.get('UVMS_PACKAGE_EVIDENCE', 'test-results/rust-optimization/package-verification.json')
output.parent.mkdir(parents=True, exist_ok=True)
output.write_text(json.dumps(result, indent=2))
print(json.dumps(result))
