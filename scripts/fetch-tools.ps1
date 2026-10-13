# Downloads yt-dlp, ffmpeg, and ffprobe into a folder, ready to bundle with
# the app. sockseek is built separately from its own repository.
#
# Usage: scripts/fetch-tools.ps1 -OutDir <output-dir> -Platform win-x64
param(
    [Parameter(Mandatory = $true)][string]$OutDir,
    [Parameter(Mandatory = $true)][ValidateSet("win-x64")][string]$Platform
)
$ErrorActionPreference = "Stop"

# ffmpeg and ffprobe come from the ffmpeg-static project (ffmpeg 6.1.1).
$ffmpegVersion = "b6.1.1"
$ffmpegBase = "https://github.com/eugeneware/ffmpeg-static/releases/download/$ffmpegVersion"
$ytdlpBase = "https://github.com/yt-dlp/yt-dlp/releases/latest/download"

$builds = @{
    "win-x64" = @{
        ffmpeg   = "ffmpeg-win32-x64"
        ffprobe  = "ffprobe-win32-x64"
        ytdlp    = "yt-dlp.exe"
    }
}

$build = $builds[$Platform]
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

curl.exe -fL --retry 3 -o "$OutDir\ffmpeg.exe"  "$ffmpegBase/$($build.ffmpeg)"
curl.exe -fL --retry 3 -o "$OutDir\ffprobe.exe" "$ffmpegBase/$($build.ffprobe)"
curl.exe -fL --retry 3 -o "$OutDir\yt-dlp.exe"  "$ytdlpBase/$($build.ytdlp)"

Write-Host "tools ready in $OutDir:"
Get-ChildItem $OutDir
