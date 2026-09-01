#!/bin/sh
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
: "${APPLE_SIGNING_IDENTITY:?APPLE_SIGNING_IDENTITY is required}"
: "${APPLE_NOTARY_PROFILE:?APPLE_NOTARY_PROFILE is required}"
: "${SOPHONOTE_UPDATER_PUBKEY:?SOPHONOTE_UPDATER_PUBKEY is required}"
: "${SOPHONOTE_UPDATER_ENDPOINT:?SOPHONOTE_UPDATER_ENDPOINT is required}"
: "${TAURI_SIGNING_PRIVATE_KEY:?TAURI_SIGNING_PRIVATE_KEY is required for updater artifacts}"
case "$APPLE_SIGNING_IDENTITY" in
  *"Developer ID Application"*) ;;
  *) echo "APPLE_SIGNING_IDENTITY must be a Developer ID Application identity" >&2; exit 2 ;;
esac
case "$SOPHONOTE_UPDATER_ENDPOINT" in
  https://*) ;;
  *) echo "SOPHONOTE_UPDATER_ENDPOINT must use HTTPS" >&2; exit 2 ;;
esac
security find-identity -v -p codesigning | grep -F "$APPLE_SIGNING_IDENTITY" >/dev/null || {
  echo "Developer ID Application identity is not available in Keychain" >&2; exit 2;
}
xcrun notarytool history --keychain-profile "$APPLE_NOTARY_PROFILE" >/dev/null

ESCAPED_ENDPOINT=$(printf '%s' "$SOPHONOTE_UPDATER_ENDPOINT" | sed 's/\\/\\\\/g; s/"/\\"/g')
ESCAPED_PUBKEY=$(printf '%s' "$SOPHONOTE_UPDATER_PUBKEY" | sed 's/\\/\\\\/g; s/"/\\"/g')
sed \
  -e "s|__APPLE_SIGNING_IDENTITY__|$APPLE_SIGNING_IDENTITY|g" \
  -e "s|__SOPHONOTE_UPDATER_PUBKEY__|$ESCAPED_PUBKEY|g" \
  -e "s|__SOPHONOTE_UPDATER_ENDPOINT__|$ESCAPED_ENDPOINT|g" \
  "$ROOT/src-tauri/tauri.release.conf.template.json" \
  > "$ROOT/src-tauri/tauri.release.conf.json"
echo "release config generated; notary profile validated"
