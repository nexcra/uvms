#!/usr/bin/env python3
"""Verify release identities and absence of actual camera credentials/vendor binaries."""
import hashlib, io, json, pathlib, shlex, tarfile, urllib.parse, zipfile

root = pathlib.Path(__file__).resolve().parents[1]
manifest=json.loads((root/'dist/java-manifest.json').read_text())
secrets=[]
for line in pathlib.Path('/etc/uvms-java/uvms.env').read_text().splitlines():
    for word in shlex.split(line, comments=True):
        if '=' in word:
            value=word.split('=',1)[1]
            if value.startswith('rtsp://'):
                password=urllib.parse.urlsplit(value).password
                if password: secrets.append(password.encode())
checked=0
def inspect(name, data):
    global checked
    checked+=1
    assert not any(s in data for s in secrets), 'Credential content detected'
    if name.endswith('.jar'):
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            for entry in archive.infolist():
                if not entry.is_dir():
                    assert not any(s in archive.read(entry) for s in secrets), 'Credential content detected in JAR'
    assert not name.lower().endswith(('.msi','.dll','.pem','.mp4','.h264')), 'Excluded vendor/media file detected'
    assert not name.endswith('/uvms.env'), 'Live environment file detected'

for artifact in manifest['artifacts']:
    path=root/'dist'/artifact['name']
    assert path.stat().st_size==artifact['bytes']
    assert hashlib.sha256(path.read_bytes()).hexdigest()==artifact['sha256']
    if path.name.endswith('.zip'):
        with zipfile.ZipFile(path) as archive:
            for entry in archive.infolist():
                if not entry.is_dir(): inspect(entry.filename, archive.read(entry))
    else:
        with tarfile.open(path) as archive:
            for entry in archive:
                if entry.isfile():
                    data=archive.extractfile(entry).read()
                    inspect(entry.name,data)
                    if entry.name.endswith('/uvms-java.jar'):
                        assert hashlib.sha256(data).hexdigest()==manifest['jar_sha256']
assert hashlib.sha256(pathlib.Path('/opt/uvms-java/uvms-java.jar').read_bytes()).hexdigest()==manifest['jar_sha256']
result={'passed':True,'files_inspected':checked,'deployed_jar_matches':True,'camera_credentials_absent':True,'vendor_binaries_absent':True,'artifacts':manifest['artifacts']}
(root/'test-results/java-optimization/package-verification.json').write_text(json.dumps(result,indent=2))
print(json.dumps(result))
