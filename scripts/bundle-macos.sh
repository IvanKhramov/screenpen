#!/bin/sh
# Wraps the release binary into screenpen.app and signs it ad hoc.
# Usage: scripts/bundle-macos.sh [binary] [output dir]
# Defaults: target/release/screenpen, target/release. SCREENPEN_VERSION sets the version.
set -eu

binary=${1:-target/release/screenpen}
out=${2:-target/release}
app="$out/screenpen.app"
# CFBundleShortVersionString wants digits and dots: v0.1.0-alpha.7 -> 0.1.0
version=$(printf '%s' "${SCREENPEN_VERSION:-0.1.0}" | sed -E 's/^v//; s/[^0-9.].*$//')
[ -n "$version" ] || version=0.1.0

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/screenpen"
cp "$(dirname "$0")/../assets/screenpen.icns" "$app/Contents/Resources/screenpen.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>screenpen</string>
    <key>CFBundleIconFile</key>
    <string>screenpen</string>
    <key>CFBundleIdentifier</key>
    <string>dev.screenpen</string>
    <key>CFBundleName</key>
    <string>screenpen</string>
    <key>CFBundleDisplayName</key>
    <string>screenpen</string>
    <key>NSHumanReadableCopyright</key>
    <string>MIT License, Ivan Khramov</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST
codesign --force --sign - "$app"
echo "$app"
