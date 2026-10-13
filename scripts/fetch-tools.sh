#!/bin/sh
# Downloads yt-dlp, ffmpeg, and ffprobe into a folder, ready to bundle with
# the app. sockseek is built separately from its own repository.
#
# Usage: scripts/fetch-tools.sh <output-dir> <darwin-arm64|darwin-x64|win-x64>
set -eu

usage="usage: fetch-tools.sh <output-dir> <darwin-arm64|darwin-x64|win-x64>"
out=${1:?"$usage"}
plat=${2:?"$usage"}

# ffmpeg and ffprobe come from the ffmpeg-static project; the version tag
# covers ffmpeg 6.1.1 builds for macOS (arm64 and x64) and Windows (x64).
ffmpeg_version=b6.1.1
ffmpeg_base="https://github.com/eugeneware/ffmpeg-static/releases/download/$ffmpeg_version"
ytdlp_base="https://github.com/yt-dlp/yt-dlp/releases/latest/download"

case "$plat" in
  darwin-arm64)
    ffmpeg=ffmpeg-darwin-arm64
    ffprobe=ffprobe-darwin-arm64
    ytdlp=yt-dlp_macos
    ext=""
    ;;
  darwin-x64)
    ffmpeg=ffmpeg-darwin-x64
    ffprobe=ffprobe-darwin-x64
    ytdlp=yt-dlp_macos
    ext=""
    ;;
  win-x64)
    ffmpeg=ffmpeg-win32-x64
    ffprobe=ffprobe-win32-x64
    ytdlp=yt-dlp.exe
    ext=".exe"
    ;;
  *)
    echo "$usage" >&2
    exit 2
    ;;
esac

mkdir -p "$out"
curl -fL --retry 3 -o "$out/ffmpeg$ext"   "$ffmpeg_base/$ffmpeg"
curl -fL --retry 3 -o "$out/ffprobe$ext"  "$ffmpeg_base/$ffprobe"
curl -fL --retry 3 -o "$out/yt-dlp$ext"   "$ytdlp_base/$ytdlp"

# yt-dlp_macos is a universal binary and needs no extension on macOS.
if [ -z "$ext" ]; then
  chmod +x "$out/ffmpeg" "$out/ffprobe" "$out/yt-dlp"
fi

echo "tools ready in $out:"
ls -l "$out"
