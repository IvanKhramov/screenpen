# screenpen

Draw on top of the screen during presentations, demos and screen recordings.
A Rust port of [ScreenInk](https://github.com/volyx/ScreenInk) by volyx, ported with the author's permission.

Status: early. Windows 11, macOS and Linux (X11, Wayland with layer-shell). See [PLAN.md](PLAN.md).

## Try it on Windows

Download `screenpen-<version>-windows-x64.exe` from [Releases](https://github.com/IvanKhramov/screenpen/releases), or build it yourself:

```powershell
cargo run --release -p screenpen
```

The first start opens a short getting-started page. screenpen lives in the tray: the icon is yellow
when idle, red while drawing and purple in mouse mode. Left click toggles drawing, right click opens
the menu (whiteboard, dimming, fading, start with Windows, settings, quit).

The palette sits at the bottom of the screen: Mouse, then the drawing tools, pointers (laser,
spotlight, magnifier), colors and S/M/L widths, undo, and `⋯` for the rest (whiteboard, freeze,
screenshot, shortcuts, settings). The letter in a button's corner is its key. Shapes share one
button with a row of kinds above it; the text tool gets a row with size, bold and italic.
Drag the palette by the dots (it snaps to screen edges); double-click them to turn it vertical.
Press `?` for all shortcuts.

The settings window (tray → Settings, or `⋯` → gear): language, light or dark theme, interface
scale, colors, width, dimming, fading, every shortcut (click and press the new keys, × turns one
off, conflicts show in red), the palette buttons with a live preview, text font and style, and
snapshots. Changes apply at once. They are stored in `%APPDATA%\screenpen\settings.toml`.

| Key | Action |
|---|---|
| Ctrl+Alt+Shift+A | Start / stop drawing |
| P · H · L · A | Pen · Highlighter · Line · Arrow |
| R · O · T · N | Rectangle · Ellipse · Text · Numbered badge |
| E · V · K | Eraser · Select · Laser pointer |
| 1-9 | Color (recolors the selection when one exists) |
| `[` · `]` · Ctrl+wheel | Thinner · Thicker |
| Ctrl+- · Ctrl+= | Smaller · bigger text (while typing also Ctrl+wheel) |
| Ctrl+B · Ctrl+I | Bold · italic text |
| C | Clear |
| Ctrl+Z · Ctrl+Shift+Z | Undo · Redo |
| Ctrl+A · Ctrl+] · Delete | Select all · Bring to front · Delete the selection |
| W · S · F | Whiteboard · Spotlight (wheel resizes it) · Freeze the screen |
| M | Magnifier (wheel zooms; Windows and macOS) |
| Ctrl+S | Screenshot with the drawing: saved to Pictures\screenpen and copied |
| G · Ctrl+Alt+Shift+G | Mouse mode: the drawing stays, clicks go through · back to drawing |
| PgUp · PgDn · arrows | Sent to the window behind (flip slides) |
| Esc | Finish typing, deselect, or stop drawing |
| Ctrl+Alt+Shift+Q | Quit |

With the pen: hold Ctrl for a rectangle, Ctrl+Alt for an ellipse, Alt for an arrow.
Shift makes lines snap to 45° and shapes square. Alt with the arrow tool bends the arrow.
Hold the right mouse button (or use the pen's eraser end) to erase with any tool. Pen pressure changes the width. Double-click text with the select tool to edit it.
While typing, text edits like any text field: arrows, Home/End, Ctrl+arrows by words, Shift selects,
the mouse places the caret and selects, Ctrl+A/C/X/V, Ctrl+Z/Y inside the text, Ctrl+Enter finishes
(Cmd on a Mac).
Keys are physical: P is the pen on any keyboard layout.

The log goes to `%TEMP%\screenpen.log`.

## Try it on macOS

Download `screenpen-<version>-macos-arm64.zip` from Releases, unpack `screenpen.app` and open it
(right click → Open the first time: the app is signed ad hoc, not notarized). It lives in the menu
bar: click the icon for the menu. Settings are in `~/Library/Application Support/screenpen/settings.toml`,
screenshots go to `~/Pictures/screenpen`, the log to `$TMPDIR/screenpen.log`. Freeze and screenshots
need the Screen Recording permission. Build the app yourself with
`cargo build --release -p screenpen && scripts/bundle-macos.sh`.

## Linux

Download `screenpen-<version>-linux-x64.tar.gz` from Releases, unpack it and run `screenpen`
(copy `screenpen.desktop` to `~/.local/share/applications` for a menu entry).

- X11 needs a compositing manager for a see-through overlay; without one the screen goes black while drawing.
- KDE Plasma, Sway and Hyprland on Wayland get native overlays. Wayland has no global hotkeys:
  bind `screenpen --toggle` to a shortcut in your desktop settings, or use the tray icon.
  Freeze, the screen under a snapshot and passing PageUp/PageDown to the app behind need X11.
- GNOME on Wayland runs screenpen through XWayland; native Wayland windows may stay above the drawing.
  The tray icon on GNOME needs the AppIndicator extension.
- Settings are in `~/.config/screenpen/settings.toml`, the log in `$XDG_RUNTIME_DIR/screenpen.log`.

## License

MIT

## Snapshots

Ctrl+S saves the screen with the drawing to `Pictures/screenpen` and copies it. The settings
window's Snapshots tab lists them: Restore puts a snapshot back on screen, frozen, with the
drawing editable; PDF exports the drawing as vectors over the screen image. From a script:
`screenpen --restore <snapshot-name>`.
