#!/bin/sh
# Installs screenpen for the current user: the binary to ~/.local/bin, the icon and
# a menu entry. Run it from the unpacked folder: ./install.sh  (./install.sh --remove undoes it)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bin="$HOME/.local/bin"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"

if [ "${1:-}" = "--remove" ]; then
    "$bin/screenpen" --quit 2>/dev/null || true
    rm -f "$bin/screenpen" "$apps/screenpen.desktop" \
        "$icons/256x256/apps/screenpen.png" "$icons/512x512/apps/screenpen.png"
    echo "screenpen removed. Settings stay in ${XDG_CONFIG_HOME:-$HOME/.config}/screenpen."
    exit 0
fi

mkdir -p "$bin" "$apps" "$icons/256x256/apps" "$icons/512x512/apps"
install -m 755 "$here/screenpen" "$bin/screenpen"
install -m 644 "$here/screenpen-256.png" "$icons/256x256/apps/screenpen.png"
install -m 644 "$here/screenpen-512.png" "$icons/512x512/apps/screenpen.png"
sed "s|^Exec=.*|Exec=$bin/screenpen|" "$here/screenpen.desktop" > "$apps/screenpen.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$icons" 2>/dev/null || true
echo "screenpen installed: find it in the app menu, or run $bin/screenpen"
