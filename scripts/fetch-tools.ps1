param(
    [Parameter(Mandatory = $true)][string]$OutDir,
    [Parameter(Mandatory = $true)][ValidateSet("win-x64")][string]$Platform
)
$ErrorActionPreference = "Stop"
python "$PSScriptRoot/fetch-tools.py" $OutDir $Platform
if ($LASTEXITCODE -ne 0) { throw "Tool download or checksum verification failed" }
