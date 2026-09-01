#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
TAURI="$ROOT/node_modules/.bin/tauri"

"$SCRIPT_DIR/prepare-tauri-target.sh"
test -x "$TAURI" || { echo "tauri CLI not found: $TAURI" >&2; exit 2; }

COMMAND=${1:-}
"$TAURI" "$@"

test "$COMMAND" = build || exit 0
test "$(uname -s)" = Darwin || exit 0

isolate_macos_app() {
  SOURCE_APP=$1
  DEST_APP=$(printf '%s' "$SOURCE_APP" | sed 's|/bundle/macos/|/bundle.noindex/macos/|')
  DEST_DIR=$(dirname "$DEST_APP")
  mkdir -p "$DEST_DIR"
  if test -e "$DEST_APP"; then
    if test -x /usr/bin/find; then
      /usr/bin/find "$DEST_APP" -depth -delete
    else
      rm -rf "$DEST_APP"
    fi
  fi
  mv "$SOURCE_APP" "$DEST_APP"
  echo "bundle isolated from Spotlight: $DEST_APP"
}

find "$ROOT/src-tauri/target" -path '*/bundle/macos/SophoNote.app' ! -path '*/bundle.noindex/*' -type d 2>/dev/null \
  | while IFS= read -r source_app; do
      isolate_macos_app "$source_app"
    done
