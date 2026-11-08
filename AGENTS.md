# Notes for agents

## Versions and releases

- Every update ships as a **patch** release (0.2.0 → 0.2.1 → 0.2.2), unless the change is very large.
- When several patches have collected, the next release can be a **minor** one (0.3.0).
- **Juan decides minor and major bumps.** Ask him before you make one; do patches without asking.
- Release: merge to `main`, then `gh workflow run release.yml --ref main -f version=X.Y.Z -f draft_release=true`.
  When every job passes, publish the draft (`gh release edit vX.Y.Z --draft=false --latest`).
- Update `packaging/RELEASE_NOTES.md` (it is the release text) and the version in `ui/Cargo.toml`
  and `engine/Cargo.toml` (the app compares it with the newest release to offer the update).
- Release file names are the same for every version (`Spotseek-macOS-arm64.zip`,
  `Spotseek-Setup-windows-x64.exe`, …): the app's updater, `install.sh`, `install.ps1` and the
  Homebrew cask find them by name. Do not rename them.
- Publishing the release starts `publish.yml`: the Homebrew tap and winget are updated.

## Package managers

- **Homebrew:** the cask (`packaging/homebrew/spotseek.rb.in`) goes to the
  `lopezlav/homebrew-tap` repository. `publish.yml` pushes it when the secret
  `HOMEBREW_TAP_TOKEN` is set (a fine-grained token with "Contents: write" on that repository).
- **winget:** the package is `Lopezlav.Spotseek`. The first version is sent by hand
  (`wingetcreate new <installer URL>`, then the pull request to microsoft/winget-pkgs); after it
  is accepted, `publish.yml` sends each new version when the secret `WINGET_TOKEN` is set (a
  classic token with `public_repo`, from an account with a fork of winget-pkgs).
- Without those secrets the jobs only print a notice.

## The app

- `ui/` is the app window (Rust, Rune). It runs the engine (`engine/`, Rust) as its backend:
  `spotseek-backend --cli <url> -o <dir> --events --ask [--manual]`. See the README, "The app window".
- The Go code (`main.go`, `internal/`, `cmd/`) is only the Mixxx server now; the app does not use it.
- Headless screens: `cd ui && cargo run --release -- --snap out.png --scene <name>` (`--list-scenes`).
  `SPOTSEEK_LANG=es` renders the Spanish text.
- Texts are in English and Spanish (`Lang::t(en, es)`). Spanish is neutral with "vos":
  pegá, buscá, apretá, elegí — with the correct accents.
- Tests: `cd engine && cargo test --release`, `cd ui && cargo test --release`, and `go test ./...` (Mixxx server).
- Updates: `SPOTSEEK_UPDATE_URL=<path to a release JSON>` points the updater at a test release;
  `SPOTSEEK_NO_UPDATE_CHECK=1` turns the launch check off. Test the install on a copy of the app
  in a temp folder, never on /Applications.
- Live Soulseek tests: use 1 to 3 track links. The server bans an account for 30 minutes after
  more than 34 searches in 220 seconds, and one account can be online in one place only.
