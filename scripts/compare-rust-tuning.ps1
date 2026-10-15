# Requires the preserved v0.6.0 baseline and a built candidate. Synthetic cameras only.
param([switch]$AudioIsolation)
$ErrorActionPreference = 'Stop'
$before = '/mnt/d/workspaces/uvms/test-results/tuning-061/baseline/uvms-gateway'
$after = '/root/uvms-target/release/uvms-gateway'
$env:UVMS_BENCH_URL = 'http://127.0.0.1:8092'
$env:UVMS_BENCH_SERVICE = 'uvms-surveillance'
$env:UVMS_BENCH_SOURCE_PORT = '18554'
$env:UVMS_BENCH_WIDTH = '640'
$env:UVMS_BENCH_HEIGHT = '360'
$env:UVMS_BENCH_COUNTS = '16'
$env:UVMS_BENCH_SECONDS = '20'
$env:UVMS_BENCH_WARMUP = '3'
$env:UVMS_BENCH_MODES = 'webrtc,mse'
$env:UVMS_BENCH_CONTROLLED_START = '1'
$phases = @('baseline-a','optimized-a','optimized-b','baseline-b')
if ($AudioIsolation) {
    # Preserved experimental binaries differ only in the RTP audio payload clone/copy.
    $phases = @('audio-copy-a','audio-shared-a','audio-shared-b','audio-copy-b')
    $env:UVMS_BENCH_SECONDS = '60'
    $env:UVMS_BENCH_WARMUP = '5'
    $env:UVMS_BENCH_MODES = 'webrtc'
}
foreach ($stage in $phases) {
    $binary = if ($AudioIsolation) {
        $kind = if ($stage.StartsWith('audio-copy')) { 'copy' } else { 'shared' }
        "/mnt/d/workspaces/uvms/test-results/tuning-061/candidate-$kind-audio"
    } elseif ($stage.StartsWith('baseline')) { $before } else { $after }
    & wsl -d Ubuntu-24.04 -u root -- python3 scripts/surveillance-fixture.py $binary
    if ($LASTEXITCODE -ne 0) { throw "Fixture setup failed: $stage" }
    $env:UVMS_BENCH_OUTPUT = "test-results/tuning-061/$stage"
    $env:UVMS_BENCH_SCOPE = "Synthetic H264+AAC 640x360 15fps, 16 viewers, recording off, $stage; A-B-B-A order; audio isolation=$AudioIsolation"
    & node scripts/benchmark-viewers.mjs
    if ($LASTEXITCODE -ne 0) { throw "Viewer comparison failed: $stage" }
}
