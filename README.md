# Spotseek

A small desktop app that downloads a public Spotify **playlist**, **album**, or
**track** as mp3 files. It reads the track list from Spotify, downloads from
Soulseek with Sockseek, fills the gaps from YouTube Music, and normalizes every
file to mp3 at the bitrate you choose. FLAC is never used.

## Requirements

The app calls these tools, so they must be on the machine:

- **Sockseek** - the Soulseek downloader. A Soulseek account must be set in
  `~/.config/sockseek/sockseek.conf` (or typed in the app).
- **yt-dlp** - for the YouTube Music fallback.
- **ffmpeg** and **ffprobe** - for the mp3 normalization.

On macOS:

```bash
brew install yt-dlp ffmpeg
# Sockseek: download a release, or build the sockseek repo
```

## Install on macOS

The app is installed at `/Applications/Spotseek.app`.

The app is not signed. If macOS blocks the first start, right-click the app and
choose **Open**, or run:

```bash
xattr -dr com.apple.quarantine /Applications/Spotseek.app
```

## Build on macOS

```bash
cd spotseek-app
go install fyne.io/tools/cmd/fyne@latest   # once
go build -o /tmp/spotseek-app .
$(go env GOPATH)/bin/fyne package -os darwin -name Spotseek -appVersion 1.0.0
```

## Build on Windows

Fyne uses cgo and OpenGL, so a Windows `.exe` **cannot be cross-compiled from
macOS**. Build it on a Windows machine:

1. Install Go, a C compiler (MSYS2 mingw-w64 gcc, or TDM-GCC), and the Fyne CLI:
   ```bat
   go install fyne.io/tools/cmd/fyne@latest
   ```
2. Install the tools:
   - Sockseek (Windows release)
   - `yt-dlp.exe`
   - ffmpeg (add `ffmpeg.exe` and `ffprobe.exe` to PATH)
3. Build:
   ```bat
   fyne package -os windows -name Spotseek
   ```
   or, without the Fyne CLI:
   ```bat
   go build -ldflags -H=windowsgui -o Spotseek.exe .
   ```

The code already handles Windows: the startup `PATH` fix adds the WinGet and
scoop folders, and **Open folder** uses `explorer`. The Windows build has not
been tested on a real Windows machine.

## Command line (for testing)

```bash
Spotseek.app/Contents/MacOS/spotseek-app --cli "<spotify-url>" -o <dir>
```

Options: `--bitrate 320`, `--no-fallback`, `--limit N`, `--extract-only`.

## Layout

- `internal/spotify` - reads playlist, album, and track metadata from public
  Spotify pages. No login or developer app needed.
- `internal/ytdlp` - YouTube Music search, ranking, and download.
- `internal/pipeline` - runs Sockseek, finds missing tracks, runs the fallback,
  and normalizes to mp3.
- `main.go` - the Fyne UI and the CLI mode.
