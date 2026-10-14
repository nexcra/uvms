#!/usr/bin/env python3
"""Package native binary and reviewed source; never includes extracted vendor artifacts or secrets."""
import hashlib,json,tarfile,zipfile
from pathlib import Path
root=Path(__file__).resolve().parent.parent
dist=root/"dist";dist.mkdir(exist_ok=True)
binary=Path("/root/uvms-target/release/uvms-gateway")
runtime=["README.md","RUN-LINUX.md","THIRD_PARTY.md","LICENSE","config.example.toml",".env.example","deploy/uvms.service","deploy/Caddyfile.example","scripts/install-linux.sh"]
runtime += [str(p.relative_to(root)) for p in (root/"web").rglob("*") if p.is_file()]
runtime += ["docs/native-service-verification.md","docs/rtsp-relay.md"]
runtime.append("docs/performance.md")
runtime += ["docs/rust-optimization.md", "docs/feature-gap-2026-10-14.md", "docs/original-buffer-optimizations.md", "docs/original-shared-udp.md"]
runtime += ["docs/rust-surveillance.md", "docs/surveillance-archive.md"]
runtime += ["docs/nvidia-transcoding.md"]
runtime += [str(p.relative_to(root)) for p in (root/"licenses").iterdir() if p.is_file()]
native=dist/"uvms-native-linux-x86_64.tar.gz"
with tarfile.open(native,"w:gz") as archive:
    archive.add(binary,arcname="uvms-native/uvms-gateway")
    for name in runtime:archive.add(root/name,arcname="uvms-native/"+name)
source=[root/n for n in ["Cargo.toml","Cargo.lock","README.md","RUN-LINUX.md","THIRD_PARTY.md","LICENSE","config.example.toml",".env.example",".gitignore",".dockerignore","Dockerfile","compose.yaml","package.json","package-lock.json"]]
for directory in ["src","web","deploy","docs","ports"]:
    for p in (root/directory).rglob("*"):
        if p.is_file() and "target" not in p.parts and (p.suffix in [".rs",".cpp",".h",".md",".toml",".js",".html",".css",".yml",".yaml",".LICENSE"] or p.name in ["Cargo.lock","uvms.service","Caddyfile.example"]):
            source.append(p)
scripts=["install-linux.sh","install-test-fixture.sh","demo.sh","test-source.sh","prepare-demo.sh","browser-smoke.mjs","browser-multiview.mjs","browser-sdk-native.mjs","browser-recordings.mjs","mse-command-regression.mjs","verify-recording.py","verify-service-lifecycle.py","package-native.py","browser-camera-wall.mjs","browser-wall-layout.mjs","setup-multicamera-demo.py"]
source += [root/"scripts"/n for n in scripts]
source.append(root/"scripts/browser-local-access.mjs")
source += [root/"scripts"/n for n in ["benchmark-viewers.mjs", "sample-service.py", "benchmark-relay.py"]]
source += [root/"scripts"/n for n in ["rust-optimization-fixture.py", "verify-native-package.py", "java-shared-port-regression.mjs", "java-session-regression.mjs"]]
source += [root/"scripts"/n for n in ["surveillance-fixture.py", "verify-surveillance-archive.py", "browser-surveillance.mjs", "browser-audio.mjs", "browser-surveillance-wall.mjs"]]
source += [root/"scripts"/n for n in ["prepare-nvidia-headers.py", "nvidia-fixture.py", "verify-nvidia.py"]]
source.append(root/"scripts/verify-hls-idle.py")
source.append(root/"scripts/browser-hls-idle.mjs")
source.append(root/"scripts/hls-audio-skew-fixture.py")
source.append(root/"scripts/browser-rtc-sharing.mjs")
# Synthetic AVCC fixtures are required by the RTSP crate's compile-time tests.
source += list((root/"ports/unreal-rtsp/tests/fixtures").glob("*.avcc"))
source += [p for p in (root/"licenses").iterdir() if p.is_file()]
with zipfile.ZipFile(dist/"uvms-native-source.zip","w",compression=zipfile.ZIP_DEFLATED) as archive:
    for p in sorted(set(source)):archive.write(p,"uvms-native-source/"+str(p.relative_to(root)).replace("\\","/"))
    # Complete, unmodified codec sources for recipients of the linked binaries.
    cache=Path('/root/.cargo/registry/cache/index.crates.io-1949cf8c6b5b557f')
    for name in ['fdk-aac-0.8.0','fdk-aac-sys-0.5.0','opus-0.3.1','audiopus_sys-0.2.2','symphonia-0.5.5','symphonia-codec-aac-0.5.5','symphonia-core-0.5.5','symphonia-metadata-0.5.5']:
        archive.write(cache/(name+'.crate'),'uvms-native-source/third-party-sources/'+name+'.crate')
files=[native,dist/"uvms-native-source.zip"]
manifest={"version":"0.4.3","engine_revision":"shared-rtp-payloads-2026-10-14","ui_revision":"surveillance-1","nvidia_api_compiled":b'libnvcuvid.so.1' in binary.read_bytes(),"binary_bytes":binary.stat().st_size,"binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(),"artifacts":[{"name":p.name,"bytes":p.stat().st_size,"sha256":hashlib.sha256(p.read_bytes()).hexdigest()} for p in files]}
(dist/"native-manifest.json").write_text(json.dumps(manifest,indent=2))
(dist/"NATIVE-SHA256SUMS").write_text("".join(f'{item["sha256"]}  {item["name"]}\n' for item in manifest["artifacts"]))
print(json.dumps(manifest,indent=2))

