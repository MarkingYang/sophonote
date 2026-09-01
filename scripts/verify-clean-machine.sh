#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
DMG=${1:-}
test -n "$DMG" || {
  echo "usage: $0 /absolute/path/to/SophoNote.dmg" >&2
  exit 2
}
case "$DMG" in
  /*) ;;
  *) DMG="$PWD/$DMG" ;;
esac
test -f "$DMG" || { echo "dmg not found: $DMG" >&2; exit 2; }

MOUNT=$(mktemp -d "${TMPDIR:-/tmp}/sophonote-clean-mount.XXXXXX")
ATTACHED=0
cleanup() {
  if [ "$ATTACHED" = 1 ]; then
    hdiutil detach "$MOUNT" -quiet 2>/dev/null || true
  fi
  rm -rf "$MOUNT"
}
trap cleanup EXIT INT TERM

hdiutil verify "$DMG"
xcrun stapler validate "$DMG"
hdiutil attach "$DMG" -nobrowse -readonly -mountpoint "$MOUNT" -quiet
ATTACHED=1
APP="$MOUNT/SophoNote.app"
test -d "$APP" || { echo "SophoNote.app missing from DMG" >&2; exit 2; }

codesign --verify --deep --strict --verbose=2 "$APP"
xcrun stapler validate "$APP"
spctl --assess --type execute --verbose=4 "$APP"

if command -v hermes >/dev/null 2>&1; then
  echo "warning: global Hermes exists; binary isolation still runs with a clean PATH" >&2
fi
env -u SOPHONOTE_HERMES_GATEWAY_URL \
  -u SOPHONOTE_HERMES_GATEWAY_TOKEN \
  -u SOPHONOTE_HERMES_HOME \
  "$SCRIPT_DIR/verify-release.sh" "$APP"

echo "clean-machine binary gate passed; complete the signed-in Provider/Session/Keychain/update data-preservation scenario from architecture §17.6"
