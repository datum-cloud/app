#!/usr/bin/env bash
# Build dmg-background.png (1×) from dmg-background@2x.png so appdmg + Finder use
# correct Retina pairing. appdmg expects the base name in JSON (e.g. dmg-background.png)
# and finds dmg-background@2x.png automatically; do not set "background" to the @2x file.
#
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$SCRIPT_DIR/dmg-background@2x.png"
DST="$SCRIPT_DIR/dmg-background.png"

if [[ ! -f "$SRC" ]]; then
  echo "Missing $SRC" >&2
  exit 1
fi

w=$(sips -g pixelWidth "$SRC" 2>/dev/null | awk '/pixelWidth/ {print $2}')
h=$(sips -g pixelHeight "$SRC" 2>/dev/null | awk '/pixelHeight/ {print $2}')
if [[ -z "$w" || -z "$h" || $((w % 2)) -ne 0 || $((h % 2)) -ne 0 ]]; then
  echo "@2x background must have even width and height; got ${w}x${h}" >&2
  exit 1
fi
sips -z $((h / 2)) $((w / 2)) "$SRC" --out "$DST" >/dev/null
echo "Wrote $DST ($((w / 2))x$((h / 2)) from ${w}x${h} @2x)"
