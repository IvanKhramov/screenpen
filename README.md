# screenpen

Draw on top of the screen during presentations, demos and screen recordings.
A Rust port of [ScreenInk](https://github.com/volyx/ScreenInk) by volyx, ported with the author's permission.

Status: early. Windows 11 first, then macOS and Linux. See [PLAN.md](PLAN.md).

## Try the Windows spike

Grab `screenpen-windows-x64` from the latest CI run, or build it yourself:

```powershell
cargo run --release -p screenpen
```

| Key | Action |
|---|---|
| Ctrl+Alt+Shift+A | Start / stop drawing |
| 1-5 | Color |
| Z · C | Undo last stroke · Clear |
| G · Ctrl+Alt+Shift+G | Ghost mode on (clicks go through) · off |
| PgUp · PgDn · arrows | Sent to the window behind (flip slides) |
| F8 | Switch how slide keys are sent (PostMessage / SendInput) |
| F9 | Hide the palette from screen capture |
| Esc | Stop drawing |
| Ctrl+Alt+Shift+Q | Quit |

The log goes to `%TEMP%\screenpen.log`.

## License

MIT
