param([Parameter(Mandatory)][string]$Installer, [Parameter(Mandatory)][string]$Log, [int]$Rounds = 14)
# Windows Defender soak before a release is published: a fresh copy of the
# installer is read every 45 s, then whatever Defender detected since the
# start is listed. Read-only: nothing is run or installed.
#   powershell -File tools/soak.ps1 -Installer <setup.exe> -Log <soak.log>
$ErrorActionPreference = 'Continue'
$start = Get-Date
"start $start  $Installer" | Out-File -FilePath $Log -Encoding utf8
$dir = Join-Path (Split-Path $Log) 'soak-copies'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
for ($i = 1; $i -le $Rounds; $i++) {
    $copy = Join-Path $dir ("copy-$i.exe")
    try {
        Copy-Item -LiteralPath $Installer -Destination $copy -Force -ErrorAction Stop
        $h = (Get-FileHash -LiteralPath $copy -Algorithm SHA256 -ErrorAction Stop).Hash
        "$(Get-Date -Format HH:mm:ss) round $i read ok $h" | Out-File -FilePath $Log -Append -Encoding utf8
    } catch {
        "$(Get-Date -Format HH:mm:ss) round $i FAILED: $($_.Exception.Message)" | Out-File -FilePath $Log -Append -Encoding utf8
    }
    Start-Sleep -Seconds 45
}
$hits = @(Get-MpThreatDetection -ErrorAction SilentlyContinue | Where-Object { $_.InitialDetectionTime -ge $start })
"detections since start: $($hits.Count)" | Out-File -FilePath $Log -Append -Encoding utf8
foreach ($d in $hits) { "  $($d.InitialDetectionTime) $($d.ThreatID) $($d.Resources -join ', ')" | Out-File -FilePath $Log -Append -Encoding utf8 }
Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
"end $(Get-Date)" | Out-File -FilePath $Log -Append -Encoding utf8
