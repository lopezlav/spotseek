Paste a public Spotify track, album, or playlist link, or search YouTube Music
without signing in. Soulseek downloads use your own account; YouTube fallback
and YouTube search do not require one.

## Downloads

- **Apple Silicon Mac (M1 or newer):** download `Spotseek-macOS-arm64.zip`, unzip,
  and drag Spotseek into Applications.
- **Windows x64:** run the `Spotseek-Setup-*.exe` installer. The installer can install for
  your account without administrator access.
- All required tools are bundled, including Deno for YouTube extraction.
- `SHA256SUMS.txt` contains checksums for the two downloads.

These builds are unsigned (the Mac bundle has an ad-hoc signature, not Apple
notarization). macOS may require Privacy & Security → Open Anyway after the
first launch attempt. If needed, remove the quarantine attribute from the app
you downloaded: `xattr -dr com.apple.quarantine /Applications/Spotseek.app`.
Windows may show SmartScreen: More info → Run anyway.

The My Spotify tab is optional advanced setup requiring your own Spotify
Developer Client ID. You do not need it for public links or YouTube search.

Spotify page extraction and YouTube downloads depend on upstream services and
can break or encounter rate limits. Automated tests cover packaging, tool startup,
MP3 encoding, and Windows installation/uninstallation; interactive Windows use
still needs a human check. Use the app only for content you are authorized to download.

Spotseek's own code is MIT licensed. Third-party licenses, exact tool versions,
and source references are included in
the app's `licenses` directory (macOS: `Contents/Resources/licenses`).
