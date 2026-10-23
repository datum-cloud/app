#!/usr/bin/env bash
# Adds NSCameraUsageDescription to a bundled macOS app so the system shows
# a camera prompt when scanning a ticket's QR code, then re-signs the
# bundle; the Info.plist hash is part of the code signature.
#
#   add-camera-usage-description.sh Datum.app [signing identity]
#
# The identity defaults to "-" (ad-hoc) for local builds. CI passes the
# release identity.
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "usage: $0 Datum.app [signing identity]" >&2
  exit 2
fi

APP=$1
IDENTITY=${2:--}
PLIST="$APP/Contents/Info.plist"
DESCRIPTION="Datum scans QR codes that pair devices with local tunnels."

if [ ! -f "$PLIST" ]; then
  echo "no Info.plist under $APP" >&2
  exit 1
fi

/usr/libexec/PlistBuddy -c "Delete :NSCameraUsageDescription" "$PLIST" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :NSCameraUsageDescription string $DESCRIPTION" "$PLIST"

codesign --force --sign "$IDENTITY" \
  --entitlements "$(dirname "$0")/../../entitlements.plist" "$APP"
