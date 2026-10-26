# screenpen

Draw on top of the screen during presentations, demos and screen recordings.
A Rust port of [ScreenInk](https://github.com/volyx/ScreenInk) by volyx, ported with the author's permission.

Status: early. Windows 11 first, then macOS and Linux. See [PLAN.md](PLAN.md).

## Try it on Windows

Download `screenpen-<version>-windows-x64.exe` from [Releases](https://github.com/IvanKhramov/screenpen/releases), or build it yourself:

```powershell
cargo run --release -p screenpen
```

screenpen lives in the tray: the icon is yellow when idle, red while drawing and purple in ghost mode.
Left click toggles drawing, right click opens the menu (whiteboard, dimming, fading, settings, quit).

The palette shows a tooltip with the name and key of every button. Drag it by the dotted grip
(it snaps to screen edges); double-click the grip to turn it vertical. Press `?` for all shortcuts.

Settings live in `%APPDATA%\screenpen\settings.toml`: colors, palette buttons and their order,
every shortcut (an empty string turns one off), language (`auto`, `en`, `ru`). "Settings…" in the
tray opens the file; changes apply the next time you start drawing.

| Key | Action |
|---|---|
| Ctrl+Alt+Shift+A | Start / stop drawing |
| P · H · L · A | Pen · Highlighter · Line · Arrow |
| R · O · T · N | Rectangle · Ellipse · Text · Numbered badge |
| E · V | Eraser · Select |
| 1-9 | Color (recolors the selection when one exists) |
| `[` · `]` · Ctrl+wheel | Thinner · Thicker |
| C | Clear |
| Ctrl+Z · Ctrl+Shift+Z | Undo · Redo |
| Ctrl+A · Ctrl+] · Delete | Select all · Bring to front · Delete the selection |
| W | Whiteboard |
| G · Ctrl+Alt+Shift+G | Ghost mode on (clicks go through) · off |
| PgUp · PgDn · arrows | Sent to the window behind (flip slides) |
| Esc | Finish typing, deselect, or stop drawing |
| Ctrl+Alt+Shift+Q | Quit |

With the pen: hold Ctrl for a rectangle, Ctrl+Alt for an ellipse, Alt for an arrow.
Shift makes lines snap to 45° and shapes square. Alt with the arrow tool bends the arrow.
Hold the right mouse button to erase with any tool. Double-click text with the select tool to edit it.
Keys are physical: P is the pen on any keyboard layout.

The log goes to `%TEMP%\screenpen.log`.

## License

MIT
