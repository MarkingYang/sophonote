#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
SOURCE_APP=${1:-$ROOT/src-tauri/target/release/bundle.noindex/macos/SophoNote.app}
INSTALL_DIR=${SOPHONOTE_INSTALL_DIR:-/Applications}
TARGET_APP="$INSTALL_DIR/SophoNote.app"
EXPECTED_ID=com.fei.sophonote
PLIST_BUDDY=/usr/libexec/PlistBuddy

fail() {
  echo "$1" >&2
  exit 2
}

case "$SOURCE_APP" in
  /*) ;;
  *) fail "source app must be an absolute path: $SOURCE_APP" ;;
esac

case "$INSTALL_DIR" in
  /*) ;;
  *) fail "install directory must be an absolute path: $INSTALL_DIR" ;;
esac

test "$INSTALL_DIR" != / || fail "refusing to use / as the install directory"
test "$(basename "$SOURCE_APP")" = SophoNote.app || fail "release app must be named SophoNote.app"
test -d "$SOURCE_APP" || fail "app not found: $SOURCE_APP"
test -f "$SOURCE_APP/Contents/Info.plist" || fail "Info.plist missing: $SOURCE_APP"

SOURCE_ID=$($PLIST_BUDDY -c 'Print :CFBundleIdentifier' "$SOURCE_APP/Contents/Info.plist" 2>/dev/null || true)
test "$SOURCE_ID" = "$EXPECTED_ID" || fail "unexpected bundle id: ${SOURCE_ID:-missing}"

mkdir -p "$INSTALL_DIR"
STAGE_DIR=$(mktemp -d "$INSTALL_DIR/.sophonote-install.XXXXXX")
STAGED_APP="$STAGE_DIR/SophoNote.app"
BACKUP_APP="$STAGE_DIR/SophoNote.previous.app"
INSTALLED=0

cleanup() {
  if test "$INSTALLED" -eq 0 && test -d "$BACKUP_APP" && ! test -e "$TARGET_APP"; then
    mv "$BACKUP_APP" "$TARGET_APP"
  fi
  /usr/bin/find "$STAGE_DIR" -depth -delete
}
trap cleanup EXIT INT TERM

ditto "$SOURCE_APP" "$STAGED_APP"
STAGED_ID=$($PLIST_BUDDY -c 'Print :CFBundleIdentifier' "$STAGED_APP/Contents/Info.plist" 2>/dev/null || true)
test "$STAGED_ID" = "$EXPECTED_ID" || fail "staged bundle verification failed"

if test -e "$TARGET_APP"; then
  mv "$TARGET_APP" "$BACKUP_APP"
fi
mv "$STAGED_APP" "$TARGET_APP"
INSTALLED=1

VERSION=$($PLIST_BUDDY -c 'Print :CFBundleShortVersionString' "$TARGET_APP/Contents/Info.plist" 2>/dev/null || printf 'unknown')
echo "installed SophoNote.app $VERSION at $TARGET_APP"
echo "data preserved at $HOME/Library/Application Support/$EXPECTED_ID"
