## New in 0.2.2

- **Linux.** A new download, `Spotseek-linux-x64.tar.gz`: unpack it and run `install.sh`
  (app menu entry and a `spotseek` command).
- **STOP keeps what is done.** The finished songs are converted into your folder and saved in the
  history; only the rest is left out. Press STOP again to stop at once.
- **No more "stuck" look on long lists.** Soulseek allows only 34 searches every 3:40, so long
  playlists pause. Spotseek now says so ("waiting for Soulseek") and shows how long it has waited.
- The sign-in window reminds you to save the password: Soulseek cannot reset it.

## New in 0.2.1

- **Soulseek account at the first launch.** Spotseek asks you to sign in to Soulseek, and you can
  create the account right there: pick a name and a password (Soulseek has no sign-up page; a new
  name creates the account). The password is kept in the macOS Keychain or the Windows Credential
  Manager. You can skip it, but then everything comes from YouTube Music, which is slower. A red
  dot on SETTINGS shows while there is no account.
- **English and Spanish**, following the system language (or choose it in SETTINGS).
- Enter saves the settings and signs you in. "New link" is now "New download".
- A clear message when the computer's clock is wrong (secure connections then fail).

A new app window: paste a public Spotify track, album, or playlist link and
watch it search, download, and turn into a poster of your tracks. Soulseek
downloads use your own account; the YouTube fallback does not need one.

- **Track details:** click a track to see its cover, where it came from (peer,
  quality, size, speed, time), and play it with a waveform you can scrub.
- **History:** every download is saved. When a link is done, you see its tracks
  and then everything you downloaded before. Missing or moved files are gray.
- **No accidental duplicates:** tracks already in your folder are not downloaded
  again. Spotseek asks "again" or "skip" next to each one, and skips them if you
  do not answer.
- Better album downloads: a slow peer no longer shows tracks as NOT FOUND, each
  file goes to the right track, and missing tracks are searched one by one on
  Soulseek before YouTube.

The YouTube Music search and the My Spotify library are in **CLASSIC WINDOW**
(bottom right of the app).

## Downloads

- **Apple Silicon Mac (M1 or newer):** download `Spotseek-macOS-arm64.zip`, unzip,
  and drag Spotseek into Applications.
- **Windows x64:** run the `Spotseek-Setup-*.exe` installer. The installer can install for
  your account without administrator access.
- **Linux x64:** unpack `Spotseek-linux-x64.tar.gz` and run `Spotseek/install.sh`. Built on
  Ubuntu 22.04; needs a desktop with a graphics driver, ALSA, D-Bus and ICU.
- All required tools are bundled, including Deno for YouTube extraction.
- `SHA256SUMS.txt` contains checksums for the downloads.

These builds are unsigned (the Mac bundle has an ad-hoc signature, not Apple
notarization). macOS may require Privacy & Security → Open Anyway after the
first launch attempt. If needed, remove the quarantine attribute from the app
you downloaded: `xattr -dr com.apple.quarantine /Applications/Spotseek.app`.
Windows may show SmartScreen: More info → Run anyway.

The My Spotify tab (in the classic window) is optional advanced setup requiring your own Spotify
Developer Client ID. You do not need it for public links or YouTube search.

Spotify page extraction and YouTube downloads depend on upstream services and
can break or encounter rate limits. Automated tests cover packaging, tool startup,
MP3 encoding, and Windows installation/uninstallation; interactive Windows use
still needs a human check. Use the app only for content you are authorized to download.

Spotseek's own code is MIT licensed. Third-party licenses, exact tool versions,
and source references are included in
the app's `licenses` directory (macOS: `Contents/Resources/licenses`).
