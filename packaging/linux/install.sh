#!/bin/sh
# Installs Spotseek for this user: the files go to ~/.local/share/spotseek, a
# "spotseek" command to ~/.local/bin, and Spotseek to the app menu.
#   ./install.sh              install (or update)
#   ./install.sh --uninstall  remove (your downloads and settings stay)
set -e
here=$(cd "$(dirname "$0")" && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
dest="$data/spotseek"
bindir="$HOME/.local/bin"
desktop="$data/applications/spotseek.desktop"

if [ "$1" = "--uninstall" ]; then
    rm -rf "$dest" "$desktop"
    [ -L "$bindir/spotseek" ] && rm -f "$bindir/spotseek"
    echo "Spotseek is removed. Your music, history and settings are still there."
    exit 0
fi

if [ "$here" != "$dest" ]; then
    rm -rf "$dest"
    mkdir -p "$dest"
    cp -R "$here/." "$dest/"
fi
chmod +x "$dest/spotseek" "$dest/spotseek-backend" "$dest"/bin/* "$dest/install.sh"
mkdir -p "$bindir" "$(dirname "$desktop")"
ln -sf "$dest/spotseek" "$bindir/spotseek"
cat > "$desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Spotseek
Comment=Paste a Spotify link, get mp3 files
Exec="$dest/spotseek"
Icon=$dest/spotseek.png
Terminal=false
Categories=AudioVideo;Audio;Network;
DESKTOP
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$(dirname "$desktop")" >/dev/null 2>&1 || true
echo "Spotseek is installed. Open it from the app menu, or run: spotseek"
case ":$PATH:" in *":$bindir:"*) ;; *) echo "(Add $bindir to your PATH to use the spotseek command.)";; esac
