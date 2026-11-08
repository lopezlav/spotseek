#!/bin/sh
# Builds Spotseek.app with the Rune UI as the app and the engine (spotseek-backend) next to it,
# in the same layout as the release build (.github/workflows/release.yml).
# If build/bin holds the tools (scripts/fetch-tools.py), they go in
# Contents/Resources/bin; if build/notices exists, it goes in Contents/Resources/licenses.
#   ui/package-mac.sh            -> ui/target/Spotseek.app
#   ui/package-mac.sh --install  -> also replaces /Applications/Spotseek.app
#                                   (the old app goes to the Trash)
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
VERSION=${VERSION:-2.0.0}
OUT="$ROOT/ui/target/Spotseek.app"

echo "Building the engine..."
(cd "$ROOT/engine" && cargo build --release)
echo "Building the Rune UI..."
(cd "$ROOT/ui" && cargo build --release)

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"
cp "$ROOT/ui/target/release/spotseek" "$OUT/Contents/MacOS/spotseek"
cp "$ROOT/engine/target/release/spotseek-backend" "$OUT/Contents/MacOS/spotseek-backend"
if [ -d "$ROOT/build/bin" ]; then cp -R "$ROOT/build/bin" "$OUT/Contents/Resources/bin"; fi
if [ -d "$ROOT/build/notices" ]; then cp -R "$ROOT/build/notices" "$OUT/Contents/Resources/licenses"; fi

# Icon: Icon.png -> icon.icns
ICONSET=$(mktemp -d)/icon.iconset
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s "$ROOT/Icon.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d "$ROOT/Icon.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$OUT/Contents/Resources/icon.icns"

cat > "$OUT/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>spotseek</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundleIdentifier</key><string>io.github.lopezlav.spotseek</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>Spotseek</string>
  <key>CFBundleDisplayName</key><string>Spotseek</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc signature, so macOS runs it on this Mac.
codesign --force --deep --sign - "$OUT"
echo "Built $OUT"

if [ "${1:-}" = "--install" ]; then
  if [ -d /Applications/Spotseek.app ]; then
    OLD=$(defaults read /Applications/Spotseek.app/Contents/Info.plist CFBundleShortVersionString 2>/dev/null || echo old)
    DEST="$HOME/.Trash/Spotseek-$OLD-$(date +%Y%m%d%H%M%S).app"
    mv /Applications/Spotseek.app "$DEST"
    echo "Moved the old app to $DEST"
  fi
  ditto "$OUT" /Applications/Spotseek.app
  echo "Installed /Applications/Spotseek.app ($VERSION)"
fi
