# Notes for agents

## Versions and releases

- Every update ships as a **patch** release (0.2.0 → 0.2.1 → 0.2.2), unless the change is very large.
- When several patches have collected, the next release can be a **minor** one (0.3.0).
- **Juan decides minor and major bumps.** Ask him before you make one; do patches without asking.
- Release: merge to `main`, then `gh workflow run release.yml --ref main -f version=X.Y.Z -f draft_release=true`.
  When every job passes, publish the draft (`gh release edit vX.Y.Z --draft=false --latest`).
- Update `packaging/RELEASE_NOTES.md` (it is the release text) and the version in `ui/Cargo.toml`.

## The app

- `ui/` is the app window (Rust, Rune). It runs the engine (`engine/`, Rust) as its backend:
  `spotseek-backend --cli <url> -o <dir> --events --ask [--manual]`. See the README, "The app window".
- The Go code (`main.go`, `internal/`, `cmd/`) is only the Mixxx server now; the app does not use it.
- Headless screens: `cd ui && cargo run --release -- --snap out.png --scene <name>` (`--list-scenes`).
  `SPOTSEEK_LANG=es` renders the Spanish text.
- Texts are in English and Spanish (`Lang::t(en, es)`). Spanish is neutral with "vos":
  pegá, buscá, apretá, elegí — with the correct accents.
- Tests: `cd engine && cargo test --release`, `cd ui && cargo test --release`, and `go test ./...` (Mixxx server).
- Live Soulseek tests: use 1 to 3 track links. The server bans an account for 30 minutes after
  more than 34 searches in 220 seconds, and one account can be online in one place only.
