#!/usr/bin/env bash
# Build Datum-dmg.icns from the same rounded PNG used for the Linux AppImage
# (assets/bundle/linux/1024.png — see Dioxus.toml [bundle] icon list).
# Use this for appdmg volume icon and Finder’s .dmg file icon.
#
# Writes: ui/packaging/dmg/Datum-dmg.icns (next to appdmg.json)
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UI_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SRC_PNG="$UI_ROOT/assets/bundle/linux/1024.png"
OUT_ICNS="$SCRIPT_DIR/Datum-dmg.icns"
# iconutil requires a directory whose name ends in .iconset
ICONSET="$SCRIPT_DIR/Datum-dmg.iconset"

cleanup() { rm -rf "$ICONSET"; }
trap cleanup EXIT
rm -rf "$ICONSET"
mkdir -p "$ICONSET"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This script only runs on macOS." >&2
  exit 1
fi

if [[ ! -f "$SRC_PNG" ]]; then
  echo "Source PNG not found: $SRC_PNG" >&2
  exit 1
fi

if ! command -v iconutil >/dev/null 2>&1; then
  echo "iconutil not found (need Xcode / Command Line Tools)." >&2
  exit 1
fi

mk() { sips -z "$2" "$3" "$SRC_PNG" --out "$ICONSET/$1" >/dev/null; }

mk icon_16x16.png 16 16
mk icon_16x16@2x.png 32 32
mk icon_32x32.png 32 32
mk icon_32x32@2x.png 64 64
mk icon_128x128.png 128 128
mk icon_128x128@2x.png 256 256
mk icon_256x256.png 256 256
mk icon_256x256@2x.png 512 512
mk icon_512x512.png 512 512
mk icon_512x512@2x.png 1024 1024

iconutil -c icns "$ICONSET" -o "$OUT_ICNS"
echo "Wrote $OUT_ICNS"
