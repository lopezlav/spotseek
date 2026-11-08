#!/usr/bin/env bash
# build-ffmpeg.sh: build the small LGPL ffmpeg that Spotseek ships.
#
# Usage:  scripts/build-ffmpeg.sh <platform> <out-dir>
#
#   platform   where to run it
#   ---------  ---------------------------------------------------------------
#   darwin-arm64  macOS (Apple Silicon). From an Intel Mac it cross-compiles.
#   darwin-x64    macOS (Intel runner). From Apple Silicon it cross-compiles.
#   linux-x64     Ubuntu 22.04. Fully static binary (-static).
#   win-x64       Ubuntu 22.04 with the apt package `mingw-w64` (cross-compile).
#
# It writes <out-dir>/ffmpeg (ffmpeg.exe for win-x64). There is no ffprobe
# and no ffplay.
#
# What is in it
#   - FFmpeg, LAME (libmp3lame) and zlib. The sources are downloaded from
#     pinned URLs and checked with sha256. All three are built static.
#   - LGPL only: there is no --enable-gpl and no --enable-nonfree.
#   - --disable-everything, then only the parts that the engine and yt-dlp
#     use (see the lists below): decode the usual audio formats, keep the
#     cover art (stream copy), encode MP3 with LAME, and the yt-dlp fixups
#     (`-c copy -f mp4`, the aac_adtstoasc filter).
#   - --disable-autodetect: it links only to system libraries (libSystem on
#     macOS, the Windows DLLs that every Windows has, nothing on Linux).
#   - No lavfi, no network, no hardware codecs, no assembler (nasm is not
#     needed; the speed of audio decoding does not change much).
#   - The mjpeg, png and bmp decoders are needed to keep the cover of MP3
#     (ID3) and M4A files: ffmpeg decodes one frame of the picture to know
#     its size, else the MP3 muxer stops with "dimensions not set". (A FLAC
#     picture block gives the size, so FLAC covers do not need them.)
#     zlib is needed by the PNG decoder.
#
# Tools that must be on the machine
#   macOS:   Xcode command line tools, curl.
#   Linux:   build-essential, curl, xz-utils.
#   Windows: the same as Linux, plus mingw-w64.
#   No nasm, no pkg-config.
#
# Environment (optional)
#   FFMPEG_WORK_DIR   build directory (default: a new temporary directory).
#                     The downloads are kept there, so you can use it as a
#                     cache between runs.
#   JOBS              parallel make jobs (default: number of CPUs).
#
# To add a format: add its decoder/demuxer/parser below, then run the build
# and the conversion tests again.

set -euo pipefail

FFMPEG_VERSION=7.1.5
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz"
FFMPEG_SHA256=de668509caf9e35e3cd162473441fdb29538c6d96ed080292b3cf9e6fc5d558f

LAME_VERSION=3.100
LAME_URL="http://deb.debian.org/debian/pool/main/l/lame/lame_${LAME_VERSION}.orig.tar.gz"
LAME_SHA256=ddfe36cab873794038ae2c1210557ad34857a4b6bdc515785d1da9e175b1da1e

ZLIB_VERSION=1.3.1
ZLIB_URL="https://zlib.net/fossils/zlib-${ZLIB_VERSION}.tar.gz"
ZLIB_SHA256=9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23

# ---- What the app uses ------------------------------------------------------

DECODERS=(
  mp3float mp3 flac alac aac vorbis opus wmav1 wmav2 ape wavpack
  pcm_s8 pcm_u8 pcm_s16le pcm_s16be pcm_s24le pcm_s24be pcm_s32le pcm_s32be
  pcm_f32le pcm_f32be pcm_f64le pcm_f64be pcm_alaw pcm_mulaw
  mjpeg png bmp
)
DEMUXERS=(mp3 flac wav aiff ape wv mov ogg matroska asf aac mpegts)
MUXERS=(mp3 mp4 ipod mov null)
ENCODERS=(libmp3lame)
PARSERS=(mpegaudio flac aac opus vorbis mjpeg png bmp)
BSFS=(aac_adtstoasc null)
FILTERS=(abuffer abuffersink aformat anull aresample)
PROTOCOLS=(file pipe)

# ---- Arguments -----------------------------------------------------------------

if [ $# -ne 2 ]; then
  echo "usage: $0 <darwin-arm64|darwin-x64|linux-x64|win-x64> <out-dir>" >&2
  exit 2
fi
PLATFORM=$1
OUT_DIR=$2

case "$(uname -s)" in
  Darwin) NCPU=$(sysctl -n hw.ncpu) ;;
  *) NCPU=$(nproc 2>/dev/null || echo 2) ;;
esac
JOBS=${JOBS:-$NCPU}

WORK=${FFMPEG_WORK_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/build-ffmpeg.XXXXXX")}
mkdir -p "$WORK/dl" "$OUT_DIR"
WORK=$(cd "$WORK" && pwd)
OUT_DIR=$(cd "$OUT_DIR" && pwd)
SRC="$WORK/src-$PLATFORM"
PREFIX="$WORK/prefix-$PLATFORM"
rm -rf "$SRC" "$PREFIX"
mkdir -p "$SRC" "$PREFIX/include" "$PREFIX/lib"

# ---- Platform settings ---------------------------------------------------------

EXE=""
CROSS=()          # extra ffmpeg configure flags for a cross build
LAME_HOST=()      # --host for the LAME configure script
AR=ar
RANLIB=ranlib
LDFLAGS=""

case "$PLATFORM" in
  darwin-arm64 | darwin-x64)
    [ "$(uname -s)" = Darwin ] || { echo "error: $PLATFORM must be built on macOS" >&2; exit 1; }
    if [ "$PLATFORM" = darwin-arm64 ]; then ARCH=arm64; FFARCH=aarch64; GNU_HOST=aarch64-apple-darwin
    else ARCH=x86_64; FFARCH=x86_64; GNU_HOST=x86_64-apple-darwin; fi
    CC="clang -arch $ARCH"
    CFLAGS="-mmacosx-version-min=11.0"
    # configure always adds CoreFoundation/CoreMedia/CoreVideo, but this
    # build uses none of them: drop the unused links.
    LDFLAGS="-mmacosx-version-min=11.0 -Wl,-dead_strip_dylibs"
    LAME_HOST=(--host="$GNU_HOST")
    if [ "$(uname -m)" != "$ARCH" ]; then
      CROSS=(--enable-cross-compile --target-os=darwin --arch="$FFARCH")
    fi
    ;;
  linux-x64)
    [ "$(uname -s)" = Linux ] || { echo "error: linux-x64 must be built on Linux" >&2; exit 1; }
    CC=gcc
    CFLAGS=""
    LDFLAGS="-static"
    ;;
  win-x64)
    [ "$(uname -s)" = Linux ] || { echo "error: win-x64 is cross-compiled on Linux" >&2; exit 1; }
    MINGW=x86_64-w64-mingw32
    CC="$MINGW-gcc"
    AR="$MINGW-ar"
    RANLIB="$MINGW-ranlib"
    CFLAGS=""
    # Static libgcc and winpthreads: the exe needs no extra DLL.
    LDFLAGS="-static -static-libgcc"
    LAME_HOST=(--host="$MINGW")
    CROSS=(--enable-cross-compile --cross-prefix="$MINGW-" --target-os=mingw32 --arch=x86_64)
    EXE=.exe
    ;;
  *)
    echo "error: unknown platform '$PLATFORM'" >&2
    exit 2
    ;;
esac

command -v ${CC%% *} >/dev/null || { echo "error: compiler '${CC%% *}' not found" >&2; exit 1; }

# ---- Download and check --------------------------------------------------------

sha256_of() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# fetch <url> <sha256> <file name>: download once into $WORK/dl, check the hash.
fetch() {
  local url=$1 want=$2 file="$WORK/dl/$3" got
  if [ ! -f "$file" ] || [ "$(sha256_of "$file")" != "$want" ]; then
    echo "==> download $url"
    curl -fL --retry 3 --retry-delay 5 -o "$file.part" "$url"
    mv "$file.part" "$file"
  fi
  got=$(sha256_of "$file")
  if [ "$got" != "$want" ]; then
    echo "error: sha256 of $file is $got, expected $want" >&2
    rm -f "$file"
    exit 1
  fi
}

fetch "$FFMPEG_URL" "$FFMPEG_SHA256" "ffmpeg-$FFMPEG_VERSION.tar.xz"
fetch "$LAME_URL" "$LAME_SHA256" "lame-$LAME_VERSION.tar.gz"
fetch "$ZLIB_URL" "$ZLIB_SHA256" "zlib-$ZLIB_VERSION.tar.gz"

tar -xJf "$WORK/dl/ffmpeg-$FFMPEG_VERSION.tar.xz" -C "$SRC"
tar -xzf "$WORK/dl/lame-$LAME_VERSION.tar.gz" -C "$SRC"
tar -xzf "$WORK/dl/zlib-$ZLIB_VERSION.tar.gz" -C "$SRC"

# ---- zlib (static) -------------------------------------------------------------
# Compiled by hand: the zlib configure script does not know mingw, and this
# is the same on every platform.

echo "==> zlib $ZLIB_VERSION"
(
  cd "$SRC/zlib-$ZLIB_VERSION"
  objs=()
  for f in adler32 compress crc32 deflate gzclose gzlib gzread gzwrite \
           infback inffast inflate inftrees trees uncompr zutil; do
    $CC $CFLAGS -O2 -DHAVE_UNISTD_H -c "$f.c" -o "$f.o"
    objs+=("$f.o")
  done
  $AR rcs "$PREFIX/lib/libz.a" "${objs[@]}"
  $RANLIB "$PREFIX/lib/libz.a"
  cp zlib.h zconf.h "$PREFIX/include/"
)

# ---- LAME (static libmp3lame only) ---------------------------------------------

echo "==> lame $LAME_VERSION"
(
  cd "$SRC/lame-$LAME_VERSION"
  # LAME 3.100 exports lame_init_old, which no longer exists. This breaks
  # some linkers; remove it from the symbol list.
  sed -i.orig '/^lame_init_old$/d' include/libmp3lame.sym
  CC="$CC" AR="$AR" RANLIB="$RANLIB" CFLAGS="$CFLAGS -O2" LDFLAGS="$LDFLAGS" \
    ./configure --prefix="$PREFIX" ${LAME_HOST[@]+"${LAME_HOST[@]}"} \
      --enable-static --disable-shared --disable-frontend --disable-decoder \
      --disable-gtktest --disable-dependency-tracking >/dev/null
  make -j"$JOBS" >"$WORK/lame-$PLATFORM.log" 2>&1 || { tail -40 "$WORK/lame-$PLATFORM.log" >&2; exit 1; }
  make install >>"$WORK/lame-$PLATFORM.log" 2>&1
)

# ---- FFmpeg --------------------------------------------------------------------

join() { local IFS=,; echo "$*"; }

FFMPEG_FLAGS=(
  --prefix="$PREFIX"
  --pkg-config=false
  --disable-everything
  --disable-autodetect
  --disable-network
  --disable-doc
  --disable-ffplay
  --disable-ffprobe
  --disable-debug
  --disable-avdevice
  --disable-swscale
  --disable-x86asm
  --enable-small
  --enable-static
  --disable-shared
  --enable-zlib
  --enable-libmp3lame
  --enable-decoder="$(join "${DECODERS[@]}")"
  --enable-demuxer="$(join "${DEMUXERS[@]}")"
  --enable-muxer="$(join "${MUXERS[@]}")"
  --enable-encoder="$(join "${ENCODERS[@]}")"
  --enable-parser="$(join "${PARSERS[@]}")"
  --enable-bsf="$(join "${BSFS[@]}")"
  --enable-filter="$(join "${FILTERS[@]}")"
  --enable-protocol="$(join "${PROTOCOLS[@]}")"
)

echo "==> ffmpeg $FFMPEG_VERSION"
(
  cd "$SRC/ffmpeg-$FFMPEG_VERSION"
  ./configure --cc="$CC" ${CROSS[@]+"${CROSS[@]}"} \
    --extra-cflags="$CFLAGS -I$PREFIX/include" \
    --extra-ldflags="$LDFLAGS -L$PREFIX/lib" \
    "${FFMPEG_FLAGS[@]}" >"$WORK/configure-$PLATFORM.log" || {
      tail -40 ffbuild/config.log >&2
      exit 1
    }
  # Stop if configure did not turn on a part that we asked for.
  for want in libmp3lame_encoder aac_adtstoasc_bsf png_decoder mov_muxer aresample_filter; do
    grep -q "^#define CONFIG_$(echo "$want" | tr a-z A-Z) 1" config_components.h || {
      echo "error: configure did not enable $want" >&2
      exit 1
    }
  done
  grep -q '^#define CONFIG_GPL 0' config.h || { echo "error: the build is not LGPL" >&2; exit 1; }
  make -j"$JOBS" ffmpeg$EXE >"$WORK/make-$PLATFORM.log" 2>&1 || {
    tail -40 "$WORK/make-$PLATFORM.log" >&2
    exit 1
  }
  # "ffmpeg" is the stripped binary ("ffmpeg_g" has the symbols).
  cp "ffmpeg$EXE" "$OUT_DIR/ffmpeg$EXE"
)

# The license texts and the exact sources, for the app's licenses folder (LGPL: the user may
# rebuild ffmpeg from these sources with this script).
LIC="$OUT_DIR/licenses"
mkdir -p "$LIC"
cp "$SRC/ffmpeg-$FFMPEG_VERSION/COPYING.LGPLv2.1" "$LIC/ffmpeg-COPYING.LGPLv2.1.txt"
cp "$SRC/ffmpeg-$FFMPEG_VERSION/LICENSE.md" "$LIC/ffmpeg-LICENSE.md"
cp "$SRC/lame-$LAME_VERSION/COPYING" "$LIC/lame-COPYING.txt"
cp "$SRC/zlib-$ZLIB_VERSION/LICENSE" "$LIC/zlib-LICENSE.txt"
cat >"$LIC/ffmpeg-SOURCES.txt" <<EOF
ffmpeg in Spotseek is built from these sources by scripts/build-ffmpeg.sh
(https://github.com/lopezlav/spotseek/blob/main/scripts/build-ffmpeg.sh), LGPL only:
  FFmpeg $FFMPEG_VERSION  $FFMPEG_URL  sha256 $FFMPEG_SHA256
  LAME $LAME_VERSION      $LAME_URL  sha256 $LAME_SHA256
  zlib $ZLIB_VERSION      $ZLIB_URL  sha256 $ZLIB_SHA256
To use your own ffmpeg, put it in place of the one in the app's bin folder.
EOF

echo "==> $OUT_DIR/ffmpeg$EXE ($(wc -c <"$OUT_DIR/ffmpeg$EXE" | tr -d ' ') bytes)"
