# Third-party components

Spotseek distributes the following separate command-line programs in its `bin`
folder. Their licenses are independent of the license for Spotseek's own code.
The installer includes upstream license texts, build notices, exact versions,
source links, and Go module notices in `licenses` (macOS: inside the app's
`Contents/Resources` folder).

- **Sockseek**, by fiso64 and contributors: AGPL-3.0. Source and build instructions:
  <https://github.com/fiso64/sockseek>. The release workflow pins its source commit
  and builds a self-contained .NET executable without modifying its source.
- **yt-dlp**: the source project uses the Unlicense, but the standalone executables
  include third-party code and are distributed under GPL-3.0-or-later. See the
  bundled upstream third-party notices and <https://github.com/yt-dlp/yt-dlp#licensing>.
- **FFmpeg and ffprobe**: the static builds include GPL components. Upstream
  build/license notices are bundled without modification. Binary provenance and
  source references: <https://github.com/eugeneware/ffmpeg-static#sources-of-the-binaries>.
- **Deno**: MIT, with third-party components under their respective licenses.
  Source: <https://github.com/denoland/deno>.
- **Fyne and other Go dependencies**: their original notices are included under
  `licenses/go`; exact module versions are listed in `licenses/go-modules.txt`.

`packaging/tools.json` records the versions, download URLs, and SHA-256 checksums
used for each platform. Update it deliberately when refreshing the bundled tools.
