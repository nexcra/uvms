param([switch]$Worker,[switch]$LiveRtsp,[string]$OutputDirectory='D:\workspaces\uvms\evidence\runtime\webrtc-original',[string]$CaptureDirectory='D:\workspaces\uvms\evidence\runtime\rtsp-original-mta')
$ErrorActionPreference='Stop'
$dllPath='D:\workspaces\uvms\evidence\extracted\UnrealWebRTC.dll'
$expectedHash='3A283DF31CED9FDCA9CF02D367213B0BE0BF01394744D56FF85F9CE627A75287'
if((Get-FileHash -LiteralPath $dllPath -Algorithm SHA256).Hash -ne $expectedHash){throw 'Original DLL identity mismatch'}
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
if(-not $Worker){
 $probeArgs=@('-NoProfile','-MTA','-File',('"'+$PSCommandPath+'"'),'-Worker','-OutputDirectory',('"'+$OutputDirectory+'"'),'-CaptureDirectory',('"'+$CaptureDirectory+'"'))
 if($LiveRtsp){$probeArgs+='-LiveRtsp'}
 $p=Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList $probeArgs -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $OutputDirectory 'stdout.txt') -RedirectStandardError (Join-Path $OutputDirectory 'stderr.txt')
 $done=$p.WaitForExit(45000)
 if(-not $done){Stop-Process -Id $p.Id -Force;$p.WaitForExit()}
 @{target=$dllPath;sha256=$expectedHash.ToLower();capture=$CaptureDirectory;liveRtsp=[bool]$LiveRtsp;boundedMilliseconds=45000;timedOut=(-not $done);exitCode=$p.ExitCode;registryRegistration=$false;authority='controlled original DLL execution; not REA evidence'} | ConvertTo-Json | Set-Content (Join-Path $OutputDirectory 'run.json')
 Get-Content (Join-Path $OutputDirectory 'stdout.txt')
 Get-Content (Join-Path $OutputDirectory 'stderr.txt')
 exit
}
Add-Type -TypeDefinition (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'original-webrtc-probe.cs') -Raw)
[OriginalWebRtcProbe]::Run($dllPath,$CaptureDirectory,$OutputDirectory,[bool]$LiveRtsp)
