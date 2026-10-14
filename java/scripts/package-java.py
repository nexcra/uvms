#!/usr/bin/env python3
"""Build self-contained source/runtime bundles from this Java project only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import zipfile

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, default=root / 'dist')
args = parser.parse_args()
dist = args.output.resolve()
dist.mkdir(parents=True, exist_ok=True)
jar = root / 'target/uvms-java-0.1.0.jar'
if not jar.is_file():
    raise SystemExit('Run mvn package in the Java project first')
mvn = 'mvn.cmd' if os.name == 'nt' else 'mvn'
subprocess.run([mvn, '-q', 'dependency:build-classpath', '-DincludeScope=runtime',
                '-Dmdep.outputFile=target/runtime-classpath.txt'], cwd=root, check=True)
generated = root / 'target/package-licenses'
generated.mkdir(parents=True, exist_ok=True)
dependencies, notices = [], {}
for filename in (root / 'target/runtime-classpath.txt').read_text().strip().split(os.pathsep):
    dependency = Path(filename)
    dependencies.append({'jar': dependency.name, 'sha256': hashlib.sha256(dependency.read_bytes()).hexdigest()})
    with zipfile.ZipFile(dependency) as archive:
        for name in archive.namelist():
            if Path(name).name.upper().startswith(('LICENSE', 'NOTICE', 'COPYING')) and not name.endswith('/'):
                output = generated / dependency.stem / name.replace('/', '__')
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_bytes(archive.read(name))
                notices['licenses/resolved/' + output.relative_to(generated).as_posix()] = output
dependency_list = generated / 'dependencies.json'
dependency_list.write_text(json.dumps(dependencies, indent=2) + '\n')
notices['licenses/resolved/dependencies.json'] = dependency_list

# Explicit roots/extensions prevent live settings, recordings and build outputs leaking in.
source = {name: root / name for name in [
    '.gitignore', 'LICENSE', 'README.md', 'THIRD_PARTY.md', 'pom.xml',
    'config.example.json', 'install-linux.sh', 'package.json', 'package-lock.json']}
allowed = {'.java', '.js', '.mjs', '.py', '.sh', '.html', '.css', '.md', '.json', '.xml', '.service', '.txt'}
for directory in ['src', 'web', 'docs', 'deploy', 'scripts', 'licenses']:
    for path in (root / directory).rglob('*'):
        if path.is_file() and not path.is_symlink() and '__pycache__' not in path.parts:
            if path.suffix in allowed or path.name in ['LICENSE', 'COPYING', 'NOTICE']:
                source[path.relative_to(root).as_posix()] = path
runtime = {name: path for name, path in source.items()
           if name in ['LICENSE', 'README.md', 'THIRD_PARTY.md', 'config.example.json', 'install-linux.sh']
           or name.startswith(('web/', 'deploy/', 'docs/', 'licenses/'))}
runtime.update(notices)
runtime['uvms-java.jar'] = jar
standalone = {**source, **runtime}

def zip_bundle(path, files):
    with zipfile.ZipFile(path, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
        for name, file in sorted(files.items()):
            archive.write(file, 'uvms-java/' + name)

zip_bundle(dist / 'uvms-java-source.zip', source)
zip_bundle(dist / 'uvms-java-standalone.zip', standalone)
with tarfile.open(dist / 'uvms-java-linux.tar.gz', 'w:gz') as archive:
    for name, file in sorted(runtime.items()):
        info = archive.gettarinfo(str(file), 'uvms-java/' + name)
        info.uid = info.gid = 0
        info.uname = info.gname = ''
        info.mode = 0o755 if name.endswith('.sh') else 0o644
        with file.open('rb') as contents:
            archive.addfile(info, contents)
manifest = {'version': '0.1.0', 'jar_sha256': hashlib.sha256(jar.read_bytes()).hexdigest(),
            'jar_bytes': jar.stat().st_size, 'dependencies': dependencies, 'artifacts': []}
for name in ['uvms-java-source.zip', 'uvms-java-linux.tar.gz', 'uvms-java-standalone.zip']:
    path = dist / name
    manifest['artifacts'].append({'name': name, 'bytes': path.stat().st_size,
                                 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
(dist / 'java-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
(dist / 'JAVA-SHA256SUMS').write_text(''.join(f'{a["sha256"]}  {a["name"]}\n' for a in manifest['artifacts']))
print(json.dumps({k: v for k, v in manifest.items() if k != 'dependencies'}, indent=2))
