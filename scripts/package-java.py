#!/usr/bin/env python3
"""Package Java runtime/source and dependency notices, without cameras or vendor artifacts."""
import hashlib
import json
import pathlib
import subprocess
import tarfile
import zipfile

root = pathlib.Path(__file__).resolve().parents[1]
java = root / 'java'
dist = root / 'dist'
dist.mkdir(exist_ok=True)
jar = java / 'target/uvms-java-0.1.0.jar'
if not jar.is_file():
    raise SystemExit('Build java/ with mvn package first')
subprocess.run(['mvn', '-q', 'dependency:build-classpath', '-DincludeScope=runtime',
                '-Dmdep.outputFile=target/runtime-classpath.txt'], cwd=java, check=True)
licenses = java / 'target/licenses'
licenses.mkdir(exist_ok=True)
dependencies = []
for filename in (java / 'target/runtime-classpath.txt').read_text().strip().split(':'):
    dependency = pathlib.Path(filename)
    dependencies.append({'jar': dependency.name, 'sha256': hashlib.sha256(dependency.read_bytes()).hexdigest()})
    with zipfile.ZipFile(dependency) as archive:
        for name in archive.namelist():
            leaf = pathlib.PurePosixPath(name).name
            if leaf.upper().startswith(('LICENSE', 'NOTICE', 'COPYING')) and not name.endswith('/'):
                output = licenses / dependency.stem / name.replace('/', '__')
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_bytes(archive.read(name))
(licenses / 'dependencies.json').write_text(json.dumps(dependencies, indent=2))
files = {jar: 'uvms-java.jar', root / 'LICENSE': 'LICENSE'}
for name in ['README.md', 'THIRD_PARTY.md', 'config.example.json', 'install-linux.sh', 'deploy/uvms-java.service']:
    files[java / name] = name
for p in (root / 'web').iterdir():
    if p.is_file():
        files[p] = 'web/' + p.name
for p in licenses.rglob('*'):
    if p.is_file():
        files[p] = 'licenses/' + str(p.relative_to(licenses))
for p in (java / 'licenses').rglob('*'):
    if p.is_file():
        files[p] = 'licenses/upstream/' + p.relative_to(java / 'licenses').as_posix()
files[root / 'docs/java-comparison.md'] = 'docs/java-comparison.md'
optimization_docs = ['java-optimization.md', 'original-shared-udp.md', 'original-buffer-optimizations.md', 'java-optimization-worklog.md']
for name in optimization_docs:
    files[root / 'docs' / name] = 'docs/' + name
evidence = [root / 'test-results/java-comparison/environment.json']
for name in ['summary.json', 'restored-source.json', 'installation.json']:
    evidence.append(root / 'test-results/java-comparison' / name)
for directory in ['java-interrupted', 'rust-before-source-change', 'warmup-restored']:
    evidence += list((root / 'test-results/java-comparison' / directory).glob('*.json'))
for directory, filename in [('java-recording', 'result.json'), ('java-local-access', 'result.json'),
                            ('java-mse-commands', 'result.json'), ('java-sessions', 'result.json')]:
    evidence.append(root / 'test-results' / directory / filename)
for backend in ['rust', 'java']:
    for name in ['viewers.json', 'service-samples.json', 'relay.json']:
        candidate = root / 'test-results/java-comparison' / backend / name
        if candidate.is_file(): evidence.append(candidate)
for p in evidence:
    files[p] = p.relative_to(root).as_posix()
optimization_evidence = []
for name in ['summary.json', 'crypto-golden.json', 'recording.json', 'installation.json', 'validation.json']:
    optimization_evidence.append(root / 'test-results/java-optimization' / name)
for directory in ['fixture-before', 'fixture-after', 'fixture-single-socket', 'shared-port', 'deployed-shared-port', 'sessions', 'mse-commands']:
    optimization_evidence += list((root / 'test-results/java-optimization' / directory).glob('*.json'))
for p in optimization_evidence:
    files[p] = p.relative_to(root).as_posix()
runtime = dist / 'uvms-java-linux.tar.gz'
with tarfile.open(runtime, 'w:gz') as archive:
    for p, destination in files.items():
        archive.add(p, arcname='uvms-java/' + destination)
source = dist / 'uvms-java-source.zip'
sources = [root / 'LICENSE', root / 'package.json', root / 'package-lock.json']
sources += [java / n for n in ['pom.xml', 'README.md', 'THIRD_PARTY.md', 'config.example.json', 'install-linux.sh', 'deploy/uvms-java.service']]
sources += list((java / 'src').rglob('*.java'))
sources += [p for p in (java / 'licenses').rglob('*') if p.is_file()]
sources += [p for p in (root / 'web').iterdir() if p.is_file()]
sources += [root / 'docs/java-comparison.md']
sources += [root / 'docs' / name for name in optimization_docs]
sources += evidence
sources += optimization_evidence
sources += [root / 'scripts' / name for name in ['package-java.py', 'benchmark-viewers.mjs', 'benchmark-relay.py', 'sample-service.py', 'browser-smoke.mjs', 'browser-local-access.mjs', 'mse-command-regression.mjs', 'java-session-regression.mjs']]
sources += [root / 'scripts' / name for name in ['java-optimization-fixture.py', 'java-shared-port-regression.mjs', 'verify-java-optimized-recording.py', 'install-optimized-java.py', 'summarize-java-optimization.py', 'verify-java-package.py', 'CryptoGolden.java']]
with zipfile.ZipFile(source, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
    for p in sources:
        archive.write(p, 'uvms-java-source/' + p.relative_to(root).as_posix())
manifest = {'version': '0.1.0', 'jar_sha256': hashlib.sha256(jar.read_bytes()).hexdigest(),
            'dependencies': dependencies, 'artifacts': []}
for p in [runtime, source]:
    manifest['artifacts'].append({'name': p.name, 'bytes': p.stat().st_size, 'sha256': hashlib.sha256(p.read_bytes()).hexdigest()})
(dist / 'java-manifest.json').write_text(json.dumps(manifest, indent=2))
(dist / 'JAVA-SHA256SUMS').write_text(''.join(f'{a["sha256"]}  {a["name"]}\n' for a in manifest['artifacts']))
print(json.dumps({k: v for k, v in manifest.items() if k != 'dependencies'}, indent=2))
