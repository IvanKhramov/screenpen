#!/bin/sh
# Writes the winget manifests for a published release, ready for a pull request to
# microsoft/winget-pkgs (manifests/i/IvanKhramov/screenpen/<version>/).
# Usage: scripts/winget-manifest.sh v0.2.0 [output dir]
set -eu
tag=$1
version=${tag#v}
out=${2:-target/winget/$version}
repo=https://github.com/IvanKhramov/screenpen
url="$repo/releases/download/$tag/screenpen-$tag-windows-x64-setup.exe"
id=IvanKhramov.screenpen
schema=1.10.0

sha=$(curl -fsSL "$url" | sha256sum | cut -d' ' -f1 | tr a-f A-F)
mkdir -p "$out"

cat > "$out/$id.yaml" <<YAML
# yaml-language-server: \$schema=https://aka.ms/winget-manifest.version.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: $schema
YAML

cat > "$out/$id.installer.yaml" <<YAML
# yaml-language-server: \$schema=https://aka.ms/winget-manifest.installer.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $version
InstallerType: inno
Scope: user
InstallModes:
- interactive
- silent
- silentWithProgress
UpgradeBehavior: install
Installers:
- Architecture: x64
  InstallerUrl: $url
  InstallerSha256: $sha
ManifestType: installer
ManifestVersion: $schema
YAML

cat > "$out/$id.locale.en-US.yaml" <<YAML
# yaml-language-server: \$schema=https://aka.ms/winget-manifest.defaultLocale.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $version
PackageLocale: en-US
Publisher: Ivan Khramov
PublisherUrl: https://github.com/IvanKhramov
PublisherSupportUrl: $repo/issues
PackageName: screenpen
PackageUrl: $repo
License: MIT
LicenseUrl: $repo/blob/main/LICENSE
ShortDescription: Draw on top of the screen during presentations, demos and calls.
Description: |-
  screenpen draws over everything on the screen: pen, highlighter, shapes, text, laser pointer,
  spotlight and magnifier, with a palette, shortcuts for everything and a mouse mode that keeps
  the drawing while clicks go to your apps.
Tags:
- annotate
- draw
- presentation
- screen
- pen
ReleaseNotesUrl: $repo/releases/tag/$tag
ManifestType: defaultLocale
ManifestVersion: $schema
YAML

cat > "$out/$id.locale.ru-RU.yaml" <<YAML
# yaml-language-server: \$schema=https://aka.ms/winget-manifest.locale.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $version
PackageLocale: ru-RU
Publisher: Ivan Khramov
PackageName: screenpen
ShortDescription: Рисование поверх экрана на презентациях, демо и созвонах.
ManifestType: locale
ManifestVersion: $schema
YAML

echo "$out"
