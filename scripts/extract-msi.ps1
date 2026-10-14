param([string]$Msi = 'D:\workspaces\UMediaServer(x64).msi', [string]$Output = 'D:\workspaces\uvms\evidence')
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $Output | Out-Null
function Invoke-Com($Object, $Name, $Arguments) {
    $Object.GetType().InvokeMember($Name, 'InvokeMethod', $null, $Object, $Arguments)
}
function Get-Com($Object, $Name, $Arguments) {
    $Object.GetType().InvokeMember($Name, 'GetProperty', $null, $Object, $Arguments)
}
$installer = New-Object -ComObject WindowsInstaller.Installer
$database = Invoke-Com $installer 'OpenDatabase' @($Msi, 0)
foreach ($table in @('Property', 'File', 'Media', 'Directory', 'Component')) {
    $view = Invoke-Com $database 'OpenView' @("SELECT * FROM $table")
    Invoke-Com $view 'Execute' $null
    $rows = @()
    while ($null -ne ($record = Invoke-Com $view 'Fetch' $null)) {
        $values = for ($i = 1; $i -le (Get-Com $record 'FieldCount' $null); $i++) { Get-Com $record 'StringData' @($i) }
        $rows += ,$values
    }
    ConvertTo-Json -InputObject $rows -Depth 5 | Set-Content -LiteralPath (Join-Path $Output "msi-$table.json")
    Invoke-Com $view 'Close' $null
}
$cabinetNames = @(Get-Content -LiteralPath (Join-Path $Output 'msi-Media.json') -Raw | ConvertFrom-Json | ForEach-Object { $_[3].TrimStart('#') })
$view = Invoke-Com $database 'OpenView' @('SELECT Name, Data FROM _Streams')
Invoke-Com $view 'Execute' $null
$streams = @()
while ($null -ne ($record = Invoke-Com $view 'Fetch' $null)) {
    $name = Get-Com $record 'StringData' @(1)
    if ($name -notin $cabinetNames) { continue }
    $safeName = [IO.Path]::GetFileName($name) + '.cab'
    $path = Join-Path $Output $safeName
    $file = [IO.File]::Open($path, [IO.FileMode]::CreateNew)
    try {
        while ($true) {
            $chunk = Invoke-Com $record 'ReadStream' @(2, 65536, 1)
            if (!$chunk.Length) { break }
            $bytes = [byte[]]::new($chunk.Length)
            for ($i=0; $i -lt $chunk.Length; $i++) { $bytes[$i] = [byte][char]$chunk[$i] }
            $file.Write($bytes, 0, $bytes.Length)
        }
    } finally { $file.Dispose() }
    $streams += [pscustomobject]@{Name=$name; Path=$path; Sha256=(Get-FileHash -LiteralPath $path).Hash}
}
Invoke-Com $view 'Close' $null
$streams | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $Output 'cabinet-streams.json')
Get-FileHash -LiteralPath $Msi | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $Output 'msi-sha256.json')
$streams
