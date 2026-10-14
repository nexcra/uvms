param([ValidateSet('browser','transport')][string]$Mode='browser')
$ErrorActionPreference='Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
$comparisonRoot="test-results/go2rtc-comparison/$Mode"
New-Item -ItemType Directory -Force $comparisonRoot | Out-Null
$run=0
try {
    foreach($engine in @('rust','go2rtc','go2rtc','rust')) {
        $run++
        $output="$comparisonRoot/$run-$engine"
        New-Item -ItemType Directory -Force $output | Out-Null
        & wsl -d Ubuntu-24.04 -u root -- systemctl stop uvms-rust-opt-bench uvms-go2rtc-bench
        if($LASTEXITCODE -ne 0){throw 'Cannot stop fixtures'}
        $service=if($engine -eq 'rust'){'uvms-rust-opt-bench'}else{'uvms-go2rtc-bench'}
        & wsl -d Ubuntu-24.04 -u root -- systemctl start $service
        if($LASTEXITCODE -ne 0){throw 'Cannot start fixture'}
        $url=if($engine -eq 'rust'){'http://127.0.0.1:8091/healthz'}else{'http://127.0.0.1:8097/api'}
        $ready=$false
        for($i=0;$i -lt 30;$i++) {
            try { $health=Invoke-RestMethod $url; $ready=$true; break } catch {Start-Sleep -Milliseconds 250}
        }
        if(-not $ready){throw 'Fixture not healthy'}
        $before=[ordered]@{engine=$engine;mode=$Mode;startedAt=(Get-Date).ToString('o');health=$health;hostCpuPercent=(Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'").PercentProcessorTime}
        $before | ConvertTo-Json -Depth 8 | Set-Content -Encoding utf8 "$output/environment.json"
        Write-Output "START $Mode $run $engine"
        if($Mode -eq 'browser'){
            $env:UVMS_CAP_ENGINE=$engine
            $env:UVMS_CAP_COUNTS='32,64'
            $env:UVMS_CAP_SECONDS='60'
            $env:UVMS_CAP_HLS='0'
            $env:UVMS_CAP_SOFTWARE_DECODE='1'
            $env:UVMS_CAP_BROWSER_PEERS='32'
            $env:UVMS_CAP_GROUP='8'
            $env:UVMS_CAP_OUTPUT=$output
            & node scripts/benchmark-capacity.mjs | Tee-Object -FilePath "$output/stdout.log"
        }else{
            & wsl -d Ubuntu-24.04 -u root -- env "UVMS_CAP_ENGINE=$engine" UVMS_CAP_COUNTS=128,512 UVMS_CAP_SECONDS=60 UVMS_CAP_RAMP=16 "UVMS_CAP_OUTPUT=/mnt/d/workspaces/uvms/$output/transport.json" /root/uvms-target/release/examples/rtc_capacity | Tee-Object -FilePath "$output/stdout.log"
        }
        $code=$LASTEXITCODE
        [ordered]@{exitCode=$code;finishedAt=(Get-Date).ToString('o');hostCpuPercent=(Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'").PercentProcessorTime} | ConvertTo-Json | Set-Content -Encoding utf8 "$output/completion.json"
        # Quality failures remain evidence; still run the other engine with identical parameters.
        Write-Output "END $Mode $run $engine exit=$code"
    }
}finally{
    & wsl -d Ubuntu-24.04 -u root -- systemctl stop uvms-rust-opt-bench uvms-go2rtc-bench
}
