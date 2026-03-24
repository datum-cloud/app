#!/usr/bin/env bash
# Build Datum.dmg locally using the same appdmg.json as CI.
# Run from anywhere; paths are resolved from this script's location.
#
# Usage:
#   export APPLE_SIGNING_IDENTITY="Developer ID Application: … (TEAMID)"
#   ./build-dmg.sh              # expects ui/dist/Datum.app (or copies from target/dx)
#   ./build-dmg.sh --bundle     # runs dx bundle --package-types macos first
#   ./build-dmg.sh --unsigned   # omit codesign on the .dmg (layout / quick test only)
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UI_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
REPO_ROOT="$(cd "$UI_ROOT/.." && pwd)"
DMG_DIR="$SCRIPT_DIR"
export DMG_DIR
APPDMG_SPEC="$DMG_DIR/appdmg.json"
ICNS_FOR_DMG="$DMG_DIR/Datum-dmg.icns"
OUT_DMG="${OUT_DMG:-$UI_ROOT/dist/Datum.dmg}"

DO_BUNDLE=false
UNSIGNED=false

for arg in "$@"; do
  case "$arg" in
    --bundle) DO_BUNDLE=true ;;
    --unsigned) UNSIGNED=true ;;
    -h|--help)
      cat <<'EOF'
Build Datum.dmg locally using the same appdmg.json as CI.

Usage:
  export APPLE_SIGNING_IDENTITY="Developer ID Application: … (TEAMID)"
  ui/packaging/dmg/build-dmg.sh              # needs ui/dist/Datum.app (or target/dx copy)
  ui/packaging/dmg/build-dmg.sh --bundle     # dx bundle --package-types macos first
  ui/packaging/dmg/build-dmg.sh --unsigned   # no codesign on .dmg (layout test only)

Override output path: OUT_DMG=/path/out.dmg ui/packaging/dmg/build-dmg.sh
EOF
      exit 0
      ;;
    *)
      echo "Unknown option: $arg" >&2
      exit 1
      ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "appdmg only runs on macOS." >&2
  exit 1
fi

if [[ "$DO_BUNDLE" == true ]]; then
  (cd "$UI_ROOT" && dx bundle --locked --desktop --release --package-types macos)
fi

mkdir -p "$UI_ROOT/dist"
if [[ ! -d "$UI_ROOT/dist/Datum.app" ]] && [[ -d "$UI_ROOT/target/dx/Datum/release/macos/Datum.app" ]]; then
  echo "Copying Datum.app from target/dx → dist/"
  cp -R "$UI_ROOT/target/dx/Datum/release/macos/Datum.app" "$UI_ROOT/dist/"
fi

if [[ ! -d "$UI_ROOT/dist/Datum.app" ]]; then
  echo "Datum.app not found. Run from repo with a built app, e.g.:" >&2
  echo "  (cd ui && dx bundle --locked --desktop --release --package-types macos)" >&2
  echo "Or pass --bundle to this script." >&2
  exit 1
fi

if [[ ! -d "$DMG_DIR/node_modules" ]]; then
  echo "Installing npm dependencies (appdmg)…"
  (cd "$DMG_DIR" && npm ci)
fi

if [[ "$UNSIGNED" != true ]] && [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  echo "APPLE_SIGNING_IDENTITY is not set. Export it (same as CI), or use --unsigned." >&2
  exit 1
fi

echo "Building DMG .icns from Linux bundle art (rounded)…"
"$DMG_DIR/mk-dmg-icns-from-linux-png.sh"

if [[ -f "$DMG_DIR/dmg-background@2x.png" ]]; then
  echo "Syncing 1× DMG background from @2x…"
  "$DMG_DIR/sync-dmg-background-1x.sh"
fi

rm -f "$UI_ROOT/dist"/*.dmg
echo "Writing $OUT_DMG"
(cd "$REPO_ROOT" && "$DMG_DIR/node_modules/.bin/appdmg" "$APPDMG_SPEC" "$OUT_DMG")

echo "Applying Finder icon to .dmg file…"
"$DMG_DIR/apply-dmg-finder-icon.sh" "$OUT_DMG" "$ICNS_FOR_DMG"

if [[ "$UNSIGNED" != true ]]; then
  echo "Codesigning DMG…"
  codesign --force --sign "$APPLE_SIGNING_IDENTITY" --timestamp "$OUT_DMG"
fi

echo "Done: $OUT_DMG"
