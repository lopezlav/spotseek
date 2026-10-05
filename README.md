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

## Connect Spotify (read-only)

The **My Spotify** tab shows your playlists, albums, and liked songs, and
downloads them with one click. This uses the official Spotify API, read-only.

Setup, once:

1. Open https://developer.spotify.com/dashboard and create an app.
2. In the app settings, add this exact Redirect URI:
   `http://127.0.0.1:48721/callback`
   (`localhost` is not allowed by Spotify; the loopback IP is.)
3. Copy the **Client ID** (no secret is needed; the app uses PKCE).
4. In the app, open **My Spotify**, paste the Client ID, and click
   **Connect Spotify**.
5. Log in in the browser. The app saves only a refresh token, in the macOS
   Keychain (Windows Credential Manager on Windows).

Scopes requested, all read-only:
`playlist-read-private playlist-read-collaborative user-library-read user-read-private`.
The app never writes to your Spotify account.

Spotify requires the owner of the developer app to have Premium. This is a
Spotify rule, not an app setting.

## Command line (for testing)

```bash
Spotseek.app/Contents/MacOS/spotseek-app --cli "<spotify-url>" -o <dir>
```

Options: `--bitrate 320`, `--no-fallback`, `--limit N`, `--extract-only`.

Add `--timings` to log JSON `TIMING` records and include stage durations in
`_report.json`. Server downloads record these timings in each job's log.
Durations cover Soulseek process startup/login, search and peer selection,
transfer attempts, missing-file checks, both YouTube searches, candidate
metadata and ranking, each yt-dlp retry, audio transfer, MP3 encoding,
metadata/thumbnail processing, and final probing/copying/transcoding.
Parent totals include their child stages; do not add both together.
Soulseek and yt-dlp subprocess stages use observed output boundaries, so a
transfer interval can include connection or queue time. First-progress time
is recorded when progress can be attributed to a track.

For repeated, isolated measurements of several tracks, save a JSON array
containing `id`, `artist`, `title`, and `duration` (seconds), then run:

```bash
go run ./cmd/download-bench --tracks tracks.json --out /tmp/spotseek-bench-new --repeat 3
```

The output directory must be new. Each trial gets its own work and audio
folders, a timestamped `timeline.log`, and a `result.json`. `results.json`
collects completed trials even if a later trial is interrupted. Downloads
run sequentially using the configured bitrate, executables, credentials,
matching threshold, and Soulseek deadline. Supplied metadata models server
downloads; use `--fetch-metadata` to include public Spotify page extraction.
`--mode direct-youtube` skips Soulseek as an explicitly forced control;
`--mode both` runs both routes. Automatic runs retain the real fallback rules.

Fallback search reads structured results and discards channel/album browse
entries. YouTube Music and YouTube searches run together. Full metadata is
checked in groups of three, with at most four search processes across jobs.
Flat metadata chooses the initial shortlist; final scoring still uses full
metadata. More candidates are checked unless the best score reaches the
matcher's maximum (12); earlier entries are still checked to preserve ties.
Metadata checks retain a shared 180-second deadline.

For server jobs and single-track CLI runs, fallback search starts alongside
Soulseek. YouTube audio is downloaded only after Soulseek misses the track.
Pending speculative search is canceled when Soulseek succeeds or the job is
canceled. Multi-track CLI and album runs search YouTube only for missing tracks.
Timing spans can overlap; pipeline elapsed time measures the real wall time.

<!-- AI-assisted documentation begins. -->
## Mixxx server and catalogue search

Run `spotseek-app serve` to start the local service on `127.0.0.1:8765`.
The Mixxx Spotify feature uses the service for login, metadata, file lookup,
and downloads. The service does not open the Mixxx database.

- `GET /api/v1/catalog/search?q=...&type=track|album|artist&offset=...`
  returns one page of up to 10 results and `next_offset`. An offset of `-1`
  means there are no more results. Set `refresh=1` to fetch again.
- `GET /api/v1/catalog/downloaded` returns all saved completed assets for the
  active account. Each asset has track metadata, a file path, and its update time.

`internal/spotify/catalog.go` defines typed search requests, result pages, and
safe Spotify API errors. `internal/server/catalog.go` separates catalogue reads
from saved-library sync. Its cache holds up to 128 pages for five minutes and
keeps accounts separate. A canceled HTTP search cancels the Spotify read. A
rate limit returns `code: rate_limited`, a retry time, and the `Retry-After`
header. Login, access, and timeout errors have short messages.

The service saves completed assets in `.spotseek/jobs.json`. Assets stay after
old job records are removed. Existing successful jobs migrate at startup.
A failed retry does not remove a completed asset. Search downloads go into the
`Search` folder under the configured music folder. Other downloads keep their
collection folder. Mixxx builds its Downloaded list from these assets and local
links, so the list can open without the server.

Checks: `go test ./...`, `go vet ./...`, and
`go test -race ./internal/server ./internal/spotify ./internal/libraryindex`.
<!-- AI-assisted documentation ends. -->

## Layout

- `internal/spotify` - reads playlist, album, and track metadata from public
  Spotify pages. No login or developer app needed.
- `internal/ytdlp` - YouTube Music search, ranking, and download.
- `internal/pipeline` - runs Sockseek, finds missing tracks, runs the fallback,
  and normalizes to mp3.
- `main.go` - the Fyne UI and the CLI mode.
