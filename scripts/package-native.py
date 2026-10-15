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
runtime.append("docs/capacity-2026-10-14.md")
runtime += ["docs/go2rtc-comparison-2026-10-15.md", "docs/go2rtc-comparison-2026-10-15.json"]
runtime += ["docs/rust-optimization.md", "docs/feature-gap-2026-10-14.md", "docs/original-buffer-optimizations.md", "docs/original-shared-udp.md"]
runtime += ["docs/rust-surveillance.md", "docs/surveillance-archive.md"]
runtime += ["docs/surveillance-soak-2026-10-15.md", "docs/surveillance-soak-2026-10-15.json"]
runtime += ["docs/storage-control.md", "docs/storage-control.json"]
runtime += ["docs/nvidia-transcoding.md", "docs/vms-engine-audit-2026-10-15.md", "docs/vms-engine-audit-2026-10-15.json"]
runtime += [str(p.relative_to(root)) for p in (root/"licenses").iterdir() if p.is_file()]
native=dist/"uvms-native-linux-x86_64.tar.gz"
with tarfile.open(native,"w:gz") as archive:
    archive.add(binary,arcname="uvms-native/uvms-gateway")
    for name in runtime:archive.add(root/name,arcname="uvms-native/"+name)
source=[root/n for n in ["Cargo.toml","Cargo.lock","README.md","RUN-LINUX.md","THIRD_PARTY.md","LICENSE","config.example.toml",".env.example",".gitignore",".dockerignore","Dockerfile","compose.yaml","package.json","package-lock.json"]]
for directory in ["src","web","deploy","docs","ports","examples"]:
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
source.append(root/"scripts/benchmark-capacity.mjs")
source += [root/"scripts"/n for n in ["go2rtc-benchmark-fixture.py", "compare-go2rtc.ps1", "summarize-go2rtc.py"]]
source.append(root/"docs/go2rtc-comparison-2026-10-15.json")
source += [root/"scripts"/n for n in ["soak-fixture.py", "soak-surveillance.mjs", "verify-soak-recordings.py", "verify-soak-recording-labels.mjs", "verify-soak-quota.py", "verify-archive-startup.py", "summarize-soak.py"]]
source.append(root/"scripts/deploy-rust-update.py")
source.append(root/"scripts/verify-vms-operations.py")
source.append(root/"scripts/verify-storage-control.py")
source.append(root/"docs/storage-control.json")
source.append(root/"docs/vms-engine-audit-2026-10-15.json")
source.append(root/"scripts/verify-recording-disk-wait.py")
source += [root/"scripts"/n for n in ["soak-recording-gaps.mjs", "test-soak-recording-gaps.mjs"]]
source.append(root/"docs/surveillance-soak-2026-10-15.json")
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
manifest={"version":"0.5.1","engine_revision":"live-recording-budget-2026-10-15","ui_revision":"recording-state-2026-10-15","nvidia_api_compiled":b'libnvcuvid.so.1' in binary.read_bytes(),"binary_bytes":binary.stat().st_size,"binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(),"artifacts":[{"name":p.name,"bytes":p.stat().st_size,"sha256":hashlib.sha256(p.read_bytes()).hexdigest()} for p in files]}
(dist/"native-manifest.json").write_text(json.dumps(manifest,indent=2))
(dist/"NATIVE-SHA256SUMS").write_text("".join(f'{item["sha256"]}  {item["name"]}\n' for item in manifest["artifacts"]))
print(json.dumps(manifest,indent=2))

