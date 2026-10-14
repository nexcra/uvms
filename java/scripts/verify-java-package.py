#!/usr/bin/env python3
"""Verify archive integrity/layout without requiring an installed service."""
import argparse
import hashlib
import json
import re
from pathlib import Path, PurePosixPath
import tarfile
import zipfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--dist', type=Path, default=Path(__file__).resolve().parents[1] / 'dist')
args = parser.parse_args()
manifest = json.loads((args.dist / 'java-manifest.json').read_text())
checked = 0
for artifact in manifest['artifacts']:
    path = args.dist / artifact['name']
    assert path.stat().st_size == artifact['bytes'], 'Size mismatch'
    assert hashlib.sha256(path.read_bytes()).hexdigest() == artifact['sha256'], 'Hash mismatch'
    if path.suffix == '.zip':
        with zipfile.ZipFile(path) as archive:
            entries = {x.filename: archive.read(x) for x in archive.infolist() if not x.is_dir()}
    else:
        with tarfile.open(path) as archive:
            entries = {x.name: archive.extractfile(x).read() for x in archive if x.isfile()}
    for name, data in entries.items():
        parts = PurePosixPath(name).parts
        assert parts[0] == 'uvms-java' and '..' not in parts, 'Invalid archive path'
        assert not set(parts) & {'node_modules', 'recordings', 'test-results', 'evidence', '.git'}, 'Private/generated directory'
        assert parts[-1] not in {'config.json', '.env', 'uvms.env'}, 'Live settings included'
        assert not name.endswith(('.key', '.pem', '.crt', '.msi', '.dll', '.mp4', '.h264')), 'Unexpected private/vendor/media file'
        assert not re.search(rb'(?m)^-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----\r?$', data), 'Private key included'
        checked += 1
    for name in ['web/index.html', 'web/app.js', 'web/style.css', 'install-linux.sh', 'config.example.json']:
        assert 'uvms-java/' + name in entries, 'Missing runtime file: ' + name
    if artifact['name'] != 'uvms-java-source.zip':
        assert hashlib.sha256(entries['uvms-java/uvms-java.jar']).hexdigest() == manifest['jar_sha256']
    if artifact['name'] != 'uvms-java-linux.tar.gz':
        for name in ['pom.xml', 'src/main/java/uvms/Gateway.java', 'scripts/package-java.py', 'package-lock.json']:
            assert 'uvms-java/' + name in entries, 'Missing source file: ' + name
print(json.dumps({'passed': True, 'files_inspected': checked, 'artifacts': manifest['artifacts']}, indent=2))
