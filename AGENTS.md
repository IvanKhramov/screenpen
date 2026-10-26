# screenpen

Rust port of ScreenInk. `PLAN.md` holds the decisions, phases and **Status**: read it
before a new phase and update Status when one is done.

## Layout

- `crates/screenpen-core`: model, geometry, undo, tiny-skia rendering. No OS code, fully tested.
- `crates/screenpen`: the app. OS code lives only in `src/platform/<os>/`.

## Verify

- `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`.
- Windows code from Linux: `cargo clippy --target x86_64-pc-windows-msvc -p screenpen -- -D warnings`.
- The live UI can't be driven from CI: Windows behavior is checked by hand with the
  checklist in PLAN.md, using the exe from GitHub Releases.

## Release

Every build handed over for testing is a release: push a tag `vX.Y.Z` (or `vX.Y.Z-alpha.N`,
published as a prerelease) and CI attaches `screenpen-<tag>-windows-x64.exe`.
