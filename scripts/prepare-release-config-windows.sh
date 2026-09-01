#!/bin/sh
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
: "${WINDOWS_CERTIFICATE_THUMBPRINT:?WINDOWS_CERTIFICATE_THUMBPRINT is required}"
: "${SOPHONOTE_UPDATER_PUBKEY:?SOPHONOTE_UPDATER_PUBKEY is required}"
: "${SOPHONOTE_UPDATER_ENDPOINT:?SOPHONOTE_UPDATER_ENDPOINT is required}"
: "${TAURI_SIGNING_PRIVATE_KEY:?TAURI_SIGNING_PRIVATE_KEY is required for updater artifacts}"
case "$SOPHONOTE_UPDATER_ENDPOINT" in
  https://*) ;;
  *) echo "SOPHONOTE_UPDATER_ENDPOINT must use HTTPS" >&2; exit 2 ;;
esac
case "$WINDOWS_CERTIFICATE_THUMBPRINT" in
  *[!0-9A-Fa-f]*) echo "WINDOWS_CERTIFICATE_THUMBPRINT must be a hex thumbprint" >&2; exit 2 ;;
esac

ESCAPED_ENDPOINT=$(printf '%s' "$SOPHONOTE_UPDATER_ENDPOINT" | sed 's/\\/\\\\/g; s/"/\\"/g')
ESCAPED_PUBKEY=$(printf '%s' "$SOPHONOTE_UPDATER_PUBKEY" | sed 's/\\/\\\\/g; s/"/\\"/g')
ESCAPED_THUMB=$(printf '%s' "$WINDOWS_CERTIFICATE_THUMBPRINT" | sed 's/\\/\\\\/g; s/"/\\"/g')
sed \
  -e "s|__WINDOWS_CERTIFICATE_THUMBPRINT__|$ESCAPED_THUMB|g" \
  -e "s|__SOPHONOTE_UPDATER_PUBKEY__|$ESCAPED_PUBKEY|g" \
  -e "s|__SOPHONOTE_UPDATER_ENDPOINT__|$ESCAPED_ENDPOINT|g" \
  "$ROOT/src-tauri/tauri.release.windows.conf.template.json" \
  > "$ROOT/src-tauri/tauri.release.windows.conf.json"
echo "Windows release config generated"
