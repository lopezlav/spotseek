#!/bin/sh
# Installs (or updates) Spotseek on macOS or Linux:
#
#   curl -fsSL https://github.com/lopezlav/spotseek/releases/latest/download/install.sh | sh
#
# macOS: Spotseek.app goes to /Applications (or ~/Applications when /Applications cannot be
# written). Linux: the files go to ~/.local/share/spotseek, with a "spotseek" command and an app
# menu entry (the install.sh inside the download does that).
#
# SPOTSEEK_BASE_URL replaces the download address (for tests; a file:// URL works too).
set -eu

base=${SPOTSEEK_BASE_URL:-https://github.com/lopezlav/spotseek/releases/latest/download}

say() { printf '%s\n' "$*"; }
fail() { say "Spotseek: $*" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || fail "curl is needed."

os=$(uname -s)
arch=$(uname -m)
case "$arch" in
  arm64|aarch64) arch=arm64 ;;
  x86_64|amd64) arch=x64 ;;
  *) fail "this computer ($arch) is not supported." ;;
esac
case "$os" in
  Darwin) asset="Spotseek-macOS-$arch.zip" ;;
  Linux)
    [ "$arch" = x64 ] || fail "on Linux, only x86-64 computers are supported for now."
    asset="Spotseek-linux-x64.tar.gz" ;;
  *) fail "this system ($os) is not supported. On Windows, see the README." ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading $asset..."
curl -fL --retry 3 --progress-bar -o "$tmp/$asset" "$base/$asset" || fail "the download failed."
if curl -fsSL --retry 3 -o "$tmp/SHA256SUMS.txt" "$base/SHA256SUMS.txt" 2>/dev/null; then
  want=$(awk -v n="$asset" '$2 == n || $2 == "*" n { print $1 }' "$tmp/SHA256SUMS.txt")
  if command -v sha256sum >/dev/null 2>&1; then
    got=$(sha256sum "$tmp/$asset" | awk '{ print $1 }')
  else
    got=$(shasum -a 256 "$tmp/$asset" | awk '{ print $1 }')
  fi
  [ -n "$want" ] || fail "SHA256SUMS.txt has no line for $asset."
  [ "$want" = "$got" ] || fail "the download is damaged (its checksum does not match). Try again."
else
  fail "could not download SHA256SUMS.txt to check the file."
fi

if [ "$os" = Darwin ]; then
  ditto -x -k "$tmp/$asset" "$tmp/unpacked"
  dest_dir=/Applications
  if [ -n "${SPOTSEEK_APP_DIR:-}" ]; then
    dest_dir=$SPOTSEEK_APP_DIR
  elif [ ! -w "$dest_dir" ]; then
    dest_dir="$HOME/Applications"
  fi
  mkdir -p "$dest_dir"
  if pgrep -xq spotseek 2>/dev/null; then
    say "Spotseek is open: it is replaced now, and the new version starts the next time you open it."
  fi
  rm -rf "$dest_dir/Spotseek.app"
  ditto "$tmp/unpacked/Spotseek.app" "$dest_dir/Spotseek.app"
  # Not downloaded by a browser, so macOS does not ask; clear the mark in case.
  xattr -dr com.apple.quarantine "$dest_dir/Spotseek.app" 2>/dev/null || true
  say "Spotseek is installed in $dest_dir. Open it from Launchpad or Spotlight."
else
  tar -xzf "$tmp/$asset" -C "$tmp"
  sh "$tmp/Spotseek/install.sh"
fi
