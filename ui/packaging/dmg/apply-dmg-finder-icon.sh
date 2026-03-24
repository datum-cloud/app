#!/usr/bin/env bash
# Embed a custom Finder icon on the .dmg file itself (not the mounted volume).
# appdmg's "icon" only sets the volume window / mount icon; the downloaded file
# stays generic until icon data is written via NSWorkspace (resource fork + FinderInfo).
#
# Usage: apply-dmg-finder-icon.sh <path-to.dmg> <path-to.icns>
# Run AFTER appdmg creates the DMG and BEFORE codesigning the DMG.
#
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This script only runs on macOS." >&2
  exit 1
fi

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 <file.dmg> <icon.icns>" >&2
  exit 2
fi

DMG=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
ICNS=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")

if [[ ! -f "$DMG" ]]; then
  echo "DMG not found: $DMG" >&2
  exit 1
fi
if [[ ! -f "$ICNS" ]]; then
  echo ".icns not found: $ICNS" >&2
  exit 1
fi

export APPLY_DMG_TARGET="$DMG"
export APPLY_DMG_ICNS="$ICNS"

# Swift is present on GitHub Actions macOS images and developer Macs.
swift - <<'SWIFT'
import AppKit

guard let icns = ProcessInfo.processInfo.environment["APPLY_DMG_ICNS"],
      let dmg = ProcessInfo.processInfo.environment["APPLY_DMG_TARGET"] else {
    fputs("Missing APPLY_DMG_ICNS or APPLY_DMG_TARGET\n", stderr)
    exit(1)
}

guard let image = NSImage(contentsOfFile: icns) else {
    fputs("Failed to load .icns at \(icns)\n", stderr)
    exit(1)
}

if !NSWorkspace.shared.setIcon(image, forFile: dmg, options: []) {
    fputs("NSWorkspace.setIcon returned false for \(dmg)\n", stderr)
    exit(1)
}
SWIFT
