# Third-party components

Spotseek distributes the following separate command-line programs in its `bin`
folder. Their licenses are independent of the license for Spotseek's own code.
The installer includes upstream license texts, build notices, exact versions,
source links, and Rust crate notices in `licenses` (macOS: inside the app's
`Contents/Resources` folder).

- **yt-dlp**: the source project uses the Unlicense, but the standalone executables
  include third-party code and are distributed under GPL-3.0-or-later. See the
  bundled upstream third-party notices and <https://github.com/yt-dlp/yt-dlp#licensing>.
- **FFmpeg and ffprobe**: the static builds include GPL components. Upstream
  build/license notices are bundled without modification. Binary provenance and
  source references: <https://github.com/eugeneware/ffmpeg-static#sources-of-the-binaries>.
- **Deno**: MIT, with third-party components under their respective licenses.
  Source: <https://github.com/denoland/deno>.
- **Rune and other Rust dependencies** of the app window (`ui/`) and the engine
  (`engine/`): Rune is MIT
  (<https://github.com/anomalyco/rune>). Crate notices are included under
  `licenses/rust`; exact crate versions and licenses are listed in
  `licenses/rust-crates.txt`.
- **Fonts** in the app window: Anton, Inter and IBM Plex Mono, all under the SIL
  Open Font License 1.1. Their license texts are in `licenses/fonts`.

`packaging/tools.json` records the versions, download URLs, and SHA-256 checksums
used for each platform. Update it deliberately when refreshing the bundled tools.
