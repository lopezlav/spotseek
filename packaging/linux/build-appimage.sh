#!/bin/sh
# Makes Spotseek-linux-x64.AppImage from the app folder the release builds (build/linux/Spotseek:
# spotseek, spotseek-backend, bin/, licenses/, spotseek.png). One file: download, make it
# executable, run. It uses the static type-2 runtime, so the system needs no libfuse2.
#   packaging/linux/build-appimage.sh build/linux/Spotseek build/artifacts/Spotseek-linux-x64.AppImage
set -eu
src=$1
out=$2
here=$(cd "$(dirname "$0")" && pwd)
tools=$(mktemp -d)
trap 'rm -rf "$tools"' EXIT

fetch() { # url sha256 dest
  curl -fsSL --retry 3 -o "$3" "$1"
  echo "$2  $3" | sha256sum -c - >/dev/null || { echo "checksum mismatch: $1" >&2; exit 1; }
}
fetch https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage \
  ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0 "$tools/appimagetool"
fetch https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64 \
  2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d "$tools/runtime"
chmod +x "$tools/appimagetool"

app="$tools/Spotseek.AppDir"
cp -R "$src" "$app"
rm -f "$app/install.sh" "$app/README.txt"
cat > "$app/AppRun" <<'RUN'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
exec "$here/spotseek" "$@"
RUN
chmod +x "$app/AppRun"
cp "$here/spotseek.desktop" "$app/spotseek.desktop"
ln -sf spotseek.png "$app/.DirIcon"

mkdir -p "$(dirname "$out")"
# No FUSE on CI runners: the tool unpacks itself and runs.
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tools/appimagetool" --no-appstream --runtime-file "$tools/runtime" "$app" "$out"
chmod +x "$out"
echo "Built $out"
