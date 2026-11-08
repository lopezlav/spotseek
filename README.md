# Spotseek

A small desktop app that downloads music as mp3 files. Paste a public Spotify
**playlist**, **album**, or **track** link. Tracks download from Soulseek first
(also FLAC and WAV, which are made into mp3), the gaps are filled from YouTube
Music, and every file becomes an mp3 at the bitrate you choose. The sound never
goes up: a 192 kbps mp3 stays 192, it is not made a bigger file that sounds the same.

The app opens one simple window: paste a link, watch it search and download,
then get a poster of the tracks. Click a song to see the files Soulseek found for
it, pick another one, or listen to it while it downloads.

![The app window](docs/screenshots/5-downloading.png)

The app and its download engine are written in Rust. The official downloads bundle
the tools it needs (yt-dlp, ffmpeg and Deno). The only extra thing you
need is a **Soulseek account** for the Soulseek route.

## Install from the [GitHub releases page](https://github.com/lopezlav/spotseek/releases)

### macOS (Apple Silicon)

1. Download `Spotseek-macOS-arm64.zip` from the latest release.
2. Unzip it and drag **Spotseek** into `/Applications`.
3. The app is not notarized by Apple. If macOS blocks the first start, use
   **System Settings → Privacy & Security → Open Anyway**, or run this once:
   ```bash
   xattr -dr com.apple.quarantine /Applications/Spotseek.app
   ```

The download is for Apple Silicon Macs (M1 and newer). On an Intel Mac,
install the tools with Homebrew and build from source (see below).

### Windows

1. Download `Spotseek-Setup-<version>.exe` from the latest release.
2. Run it and follow the setup steps. The tools are installed with the app,
   in the `bin` folder next to the program.

Windows may show a blue SmartScreen warning because the app is not signed.
Choose **More info**, then **Run anyway**.

### Linux (x64)

1. Download `Spotseek-linux-x64.tar.gz` from the latest release.
2. Unpack it and run the install script:
   ```bash
   tar -xzf Spotseek-linux-x64.tar.gz
   ./Spotseek/install.sh
   ```
   This puts Spotseek in your app menu and adds a `spotseek` command
   (`~/.local/share/spotseek`). Or run `./Spotseek/spotseek` without installing.
   `~/.local/share/spotseek/install.sh --uninstall` removes it.

All tools are included. The system needs what a desktop normally has: a
graphics driver (Vulkan or OpenGL), ALSA, D-Bus with a secret service (GNOME
Keyring or KWallet) for the Soulseek password. Built on Ubuntu 22.04.

## Soulseek account

Downloads try Soulseek first. Spotseek has its own Soulseek client, so no other
Soulseek program is needed. At the first launch Spotseek asks for an account; Soulseek
has no sign-up page, so a new name and password there create the account.

An account in `~/.config/sockseek/sockseek.conf` (lines `username = …` and
`password = …`) also works. Without an account, every track comes from YouTube Music.

## Requirements

Nothing must be installed when you use the official downloads. If you build
the app yourself, these tools must be on the machine, because the self-built
app has no bundled copies:

- **yt-dlp** - for the YouTube Music fallback.
- **ffmpeg** - for the mp3 conversion (and to read what a file is).
- **Deno** - JavaScript runtime needed by yt-dlp for full YouTube support.

On macOS:

```bash
brew install yt-dlp ffmpeg deno
```

## The app window (Rune UI) and the engine

The window is `ui/`, written in Rust with [Rune](https://github.com/anomalyco/rune).
It does no downloading itself: it runs the engine, `engine/` (Rust, the program
`spotseek-backend`), as a child process:
`spotseek-backend --cli <url> -o <dir> --events --ask [--manual]`. The engine writes one
JSON event per line (see `engine/src/events.rs`) and stops when its stdin closes. Stdin
lines it reads: `{"only":[0,2,5]}` after the track list (with `--ask`), `{"stop":true}`
(stop downloading, keep and convert what is done), and `{"pick":{"i":3,"id":7}}` or
`{"pick":{"i":3,"yt":true}}` (download that file for track 3, or take it from YouTube
Music). The window looks for `spotseek-backend` next to its own program; set
`SPOTSEEK_BACKEND` to use another path.

How the engine works (`engine/src`):

- `slsk/` - the Soulseek client: sign-in, searches (at most 34 every 220 seconds, the
  server's limit), peer connections (direct, or through the server when a peer cannot
  reach us), and downloads.
- `rank.rs` - for each track, every file that peers send is scored: is it the same song
  (title and artist in the name, the length against Spotify's, no "remix" or "live" that
  the title does not have), is the sound good enough (lossless, or an mp3 at or above the
  bitrate you chose), and will it come fast (free upload slot, queue, the peer's speed,
  the time the file takes). The search stops early when there are enough good files.
- `store.rs` - what is kept between runs in the config folder: `peers.json` (peers that sent
  good files: how many, how fast, which artists) and `search-cache.json` (search answers, 6 hours).
- Searching less: Soulseek counts searches (34 every 220 s), never messages between peers. So
  before a song spends a search, the engine looks for it for free: in the shares of the best
  remembered peers (at the start of a run), and in the folder of a peer that has another song by the
  same artist or from the same album (the artist's folder when it is one). Songs that turn up there
  skip their search; songs waiting for the search limit give up their place when they turn up. A
  song found that way still gets its real search when those files fail.
- `pipeline.rs` - all tracks at the same time (12 downloads at once, YouTube at most 2; a peer that already sends us a file is avoided when another is nearly as good, since most peers send one file per user at a time): search, take the
  best file, and when it does not start in 15 seconds, stalls, or is much too slow while
  there are other files, take the next one. After six files, or when Soulseek has none,
  YouTube Music. Each track is converted the moment it lands.
- `convert.rs` - the sound never goes up: an mp3 at or below the bitrate you chose is
  copied as it is; a better mp3, FLAC, WAV or AIFF is encoded down to it; other lossy
  formats (YouTube's opus, AAC) are encoded at no more than their own bitrate.
- `spotify.rs`, `youtube.rs` - the public Spotify pages, and yt-dlp.

In the window:

- The headline and the liquid-metal progress bar are our own shader
  (`ui/src/liquid.wgsl`) at Rune effect slots. The headline words are signed
  distance fields (`ui/src/sdf.rs`), so one word can flow into the next.
- **Files: AUTO or I PICK** (bottom of the paste screen, and in the settings). With AUTO, the
  engine takes the best file. With I PICK, each song waits: click it to see the files Soulseek
  found, best first, each with its format and quality, size, length (in red when it is not
  Spotify's), free slot or queue, the peer's speed and the time it should take. A green dot
  means the right song from a free peer; yellow, a wait or a doubt; red, maybe another song.
  "Best file for all" picks the first one for every song. In both modes you can pick another
  file while one downloads, or "use YouTube Music".
- **Listen while it downloads.** Open a song that downloads (mp3, FLAC or WAV from Soulseek):
  the player shows what is on disk; the part that is not there yet is a faint line. Play and
  seek only go as far as the download; at its end the player waits and goes on by itself, so
  it never plays silence. When the mp3 is ready, it goes on with the mp3.
- At launch, the window asks for a Soulseek account until there is one (it can be skipped:
  then everything comes from YouTube Music). `spotseek-backend --cli --check-login --user NAME`
  (password on stdin) signs in once and prints `{"ok":…}`. The password is kept in the macOS
  Keychain, the Windows Credential Manager, or the Linux secret service.
- The texts are in English and Spanish, by the system language (`SPOTSEEK_LANG=es` to test).
- Click a track to open its details: cover, source, peer, quality, size,
  average speed, time, and a player (space plays and pauses).
- Tracks that are already in the output folder (from the history, or by their
  mp3 tags) are not downloaded again. The window asks "download again" or
  "skip" next to each one. With no answer, they are skipped when nothing else
  is left, after a countdown that only runs while the window has focus.
- Everything is in the user's config folder (`~/Library/Application
  Support/Spotseek` on macOS, `%APPDATA%\Spotseek` on Windows):
  `settings.json`, `history.json` (every download, by link), `covers/`, and
  `logs/` (each engine run, line by line; the last 40 are kept). The Soulseek
  password is never saved in a file. `SPOTSEEK_CONFIG_DIR` sets another folder.

Development helpers:

```bash
cd engine && cargo test --release && cd ..
# One run from the command line (text output; add --events for the JSON events):
engine/target/release/spotseek-backend --cli "<spotify-url>" -o /tmp/out --limit 2
cd ui
cargo test --release
# Render one screen headless (see --list-scenes: downloading, details, manual, preview, …)
cargo run --release -- --snap out.png --scene downloading --size 1100x780 --scale 2
# A scripted download with no backend and no network, recorded as video
SPOTSEEK_DEMO=1 cargo run --release -- --script scripts/demo.json --video demo.mp4 --size 1100x780
```

## Build on macOS

Rust (stable) and the Xcode command line tools are needed.

```bash
ui/package-mac.sh            # builds ui/target/Spotseek.app (window and engine)
ui/package-mac.sh --install  # also replaces /Applications/Spotseek.app (old app to the Trash)
```

To bundle the tools into the .app (Python 3 is needed on the build machine
only), run this before `package-mac.sh`:

```bash
scripts/fetch-tools.sh build/bin darwin-arm64
```

## Build on Windows

Build on a Windows machine with Rust (stable, MSVC toolchain), and put yt-dlp and
ffmpeg on the PATH (or in a `bin` folder next to the program):

```bat
cd engine && cargo build --release && cd ..
cd ui && cargo build --release && cd ..
copy ui\target\release\spotseek.exe Spotseek.exe
copy engine\target\release\spotseek-backend.exe spotseek-backend.exe
```

Keep `spotseek-backend.exe` next to `Spotseek.exe`. Release builds test silent
installation, tool startup, MP3 encoding, and uninstallation on a Windows runner.
Interactive desktop use still needs a human check.

## Releases

GitHub Actions builds the downloads whenever a tag like `v1.2.3` is pushed:

- `Spotseek-macOS-arm64.zip` - the .app with all tools bundled.
- `Spotseek-Setup-<version>.exe` - the Windows installer with all tools bundled.
- `Spotseek-linux-x64.tar.gz` - the Linux app with all tools bundled and an install script.

Each contains the app window (the Rune UI, `Spotseek.app/Contents/MacOS/spotseek`
or `Spotseek.exe`) and the engine next to it (`spotseek-backend`).

The workflow also runs manually from the Actions tab and uploads the same
files as build artifacts. Both platform builds, tests, and smoke tests must
pass before a **draft** release is created, with `SHA256SUMS.txt`. Publish the
draft from the releases page after checking the downloads.

```bash
gh workflow run release.yml --ref main -f version=0.1.0
# Build, verify, and prepare a draft from the selected commit:
gh workflow run release.yml --ref main -f version=0.1.0 -f draft_release=true
# Alternatively, pushing a v0.1.0 tag also builds a draft.
```

Downloaded tools are pinned by version and SHA-256 in `packaging/tools.json`;
changing a tool requires updating its checksum and testing a new manual build. CI
runs the tests of the engine, the app window and the Go Mixxx server on macOS,
Windows and Linux for pushes and pull requests.

Spotseek's own code is licensed under [MIT](LICENSE).
See [third-party notices](THIRD_PARTY_NOTICES.md) for bundled licenses and
source references. These are also included inside both installers.

## The Go program: the Mixxx server

The Go code in this repository (`main.go`, `internal/`, `cmd/`) is now only the local
server for the Mixxx fork (`spotseek-app serve`, below) and its old Fyne window. The app
does not use it, and the releases do not include it. Porting the server to Rust is the
next step. The server's Spotify login (read-only, PKCE) needs a Spotify developer app with
the Redirect URI `http://127.0.0.1:48721/callback`.

<!-- AI-assisted documentation begins. -->
## Mixxx server and catalogue search

The Mixxx progressive-download prototype uses `POST /api/v1/downloads/prepare`
with one track's metadata to prepare a drag without downloading audio.
`POST /api/v1/downloads` accepts `progressive: true` to expose a stable playback
descriptor, buffered duration, completion, and decoder errors in job snapshots.
Soulseek transfers decode to append-only PCM while the final MP3 is processed
separately. YouTube fallback loads after completion. See the Mixxx fork's
`docs/progressive-downloads.md` for behavior and prototype limitations.

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

- `engine` - the download engine (Rust): Soulseek client, ranking, conversion,
  Spotify and YouTube Music. It is `spotseek-backend`.
- `scripts` - downloads the bundled tools for packaging, and the smoke test.
- `packaging` - the Windows installer script, the Linux install script, the release notes.
- `main.go`, `internal`, `cmd` - the Go Mixxx server (not in the app).
- `ui` - the app window (Rust, Rune).
