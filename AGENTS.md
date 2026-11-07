# Notes for agents

## Versions and releases

- Every update ships as a **patch** release (0.2.0 → 0.2.1 → 0.2.2), unless the change is very large.
- When several patches have collected, the next release can be a **minor** one (0.3.0).
- **Juan decides minor and major bumps.** Ask him before you make one; do patches without asking.
- Release: merge to `main`, then `gh workflow run release.yml --ref main -f version=X.Y.Z -f draft_release=true`.
  When every job passes, publish the draft (`gh release edit vX.Y.Z --draft=false --latest`).
- Update `packaging/RELEASE_NOTES.md` (it is the release text) and the version in `ui/Cargo.toml`.

## The app

- `ui/` is the app window (Rust, Rune). It runs the Go program (`main.go`, `internal/`) as its backend:
  `spotseek-backend --cli <url> -o <dir> --events --ask`. See the README, "The app window".
- Headless screens: `cd ui && cargo run --release -- --snap out.png --scene <name>` (`--list-scenes`).
  `SPOTSEEK_LANG=es` renders the Spanish text.
- Texts are in English and Spanish (`Lang::t(en, es)`). Spanish is neutral with "vos":
  pegá, buscá, apretá, elegí — with the correct accents.
- Tests: `go test ./...` and `cd ui && cargo test --release`.
