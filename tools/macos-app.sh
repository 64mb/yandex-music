#!/bin/bash
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 || $2 != *.app || -e $2 ]]; then
    echo "Usage: $0 <binary> <new-output.app> [icon.icns]" >&2
    exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
app="$2"
contents="$app/Contents"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -n 1)"
mkdir -p "$contents/MacOS" "$contents/Library/LaunchAgents" "$contents/Resources"
install -m 755 "$1" "$contents/MacOS/yamusic-rs"
install -m 644 "$root/packaging/macos/"*.apple-music.plist "$contents/Library/LaunchAgents/"
install -m 644 "$root/LICENSE" "$contents/Resources/LICENSE"
if [[ $# -eq 3 ]]; then
    install -m 644 "$3" "$contents/Resources/AppIcon.icns"
else
    sips -s format icns "$root/packaging/icons/256.png" --out "$contents/Resources/AppIcon.icns" >/dev/null
fi
install -m 644 "$root/packaging/macos/Info.plist" "$contents/Info.plist"
plutil -insert CFBundleShortVersionString -string "$version" "$contents/Info.plist"
plutil -insert CFBundleVersion -string "${MACOS_BUILD_NUMBER:-$version}" "$contents/Info.plist"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
