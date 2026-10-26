# screenpen

Rust port of ScreenInk. `PLAN.md` holds the decisions, phases and **Status**: read it
before a new phase and update Status when one is done.

## Layout

- `crates/screenpen-core`: model, geometry, undo, tiny-skia rendering. No OS code, fully tested.
- `crates/screenpen`: the app. OS code lives only in `src/platform/<os>/`.

## Verify

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Rendering: `cargo run -p screenpen-core --example selftest -- <dir>` writes `annotations.png`; look at it when
  you change how something draws, and add new kinds to `examples/selftest.rs`.
- Windows code from Linux: `cargo clippy --target x86_64-pc-windows-msvc -p screenpen -- -D warnings`.
- macOS code from Linux: `rustup target add aarch64-apple-darwin`, then `cargo clippy --target aarch64-apple-darwin -p screenpen -- -D warnings`.
- The live UI can't be driven from CI: Windows behavior is checked by hand with the
  checklist in PLAN.md, using the exe from GitHub Releases.

## Release

Every build handed over for testing is a release: push a tag `vX.Y.Z` (or `vX.Y.Z-alpha.N`,
published as a prerelease) and CI attaches `screenpen-<tag>-windows-x64.exe` and `screenpen-<tag>-macos-arm64.zip`.
- Settings window: `SCREENPEN_SETTINGS_SHOT=<dir> screenpen --settings` saves a PNG of each tab and quits.
  On a headless Linux box run it under `xvfb-run` with `LIBGL_ALWAYS_SOFTWARE=1` (needs libxkbcommon-x11).
