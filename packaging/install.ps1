# Installs (or updates) Spotseek on Windows, for this user:
#
#   irm https://github.com/lopezlav/spotseek/releases/latest/download/install.ps1 | iex
#
# It downloads the installer, checks it against SHA256SUMS.txt and runs it without questions.
# $env:SPOTSEEK_BASE_URL replaces the download address (for tests).
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$base = if ($env:SPOTSEEK_BASE_URL) { $env:SPOTSEEK_BASE_URL } else { 'https://github.com/lopezlav/spotseek/releases/latest/download' }
$asset = 'Spotseek-Setup-windows-x64.exe'
if (-not [Environment]::Is64BitOperatingSystem) { throw 'Spotseek needs 64-bit Windows.' }
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("spotseek-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $exe = Join-Path $tmp $asset
    Write-Host "Downloading $asset..."
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $exe
    $sums = (Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS.txt").Content
    if ($sums -is [byte[]]) { $sums = [Text.Encoding]::UTF8.GetString($sums) }
    $line = ($sums -split "`n") | Where-Object { ($_ -split '\s+')[1] -in @($asset, "*$asset") } | Select-Object -First 1
    if (-not $line) { throw "SHA256SUMS.txt has no line for $asset." }
    $want = ($line -split '\s+')[0].ToLower()
    $got = (Get-FileHash -Algorithm SHA256 $exe).Hash.ToLower()
    if ($want -ne $got) { throw 'The download is damaged (its checksum does not match). Try again.' }
    Write-Host 'Installing...'
    $p = Start-Process -FilePath $exe -ArgumentList @('/SILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CLOSEAPPLICATIONS', '/CURRENTUSER') -PassThru -Wait
    if ($p.ExitCode -ne 0) { throw "The installer stopped with code $($p.ExitCode)." }
    Write-Host 'Spotseek is installed. Open it from the Start menu.'
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
