#!/bin/sh
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
APP=${1:-$ROOT/src-tauri/target/release/bundle.noindex/macos/SophoNote.app}
test -d "$APP" || { echo "app not found: $APP" >&2; exit 2; }
codesign --verify --deep --strict --verbose=2 "$APP"
test -x "$APP/Contents/MacOS/sophonote"
find "$APP/Contents/Resources/hermes" -name MANIFEST.toml -print -quit | grep -q .

ISOLATED_HOME=$(mktemp -d "${TMPDIR:-/tmp}/sophonote-clean-home.XXXXXX")
trap 'rm -rf "$ISOLATED_HOME"' EXIT INT TERM
env -i HOME="$ISOLATED_HOME" PATH=/usr/bin:/bin:/usr/sbin:/sbin \
  "$APP/Contents/MacOS/sophonote" >"$ISOLATED_HOME/sophonote.log" 2>&1 &
PID=$!
sleep 12
if ! kill -0 "$PID" 2>/dev/null; then
  cat "$ISOLATED_HOME/sophonote.log" >&2
  echo "SophoNote host exited before isolated smoke completed" >&2
  exit 1
fi
HERMES_PID=$(pgrep -f "$APP/Contents/Resources/hermes/.*/runtime/python/bin/python3" | head -1 || true)
test -n "$HERMES_PID" || { cat "$ISOLATED_HOME/sophonote.log" >&2; kill "$PID" || true; exit 1; }
HERMES_COMMAND=$(ps -p "$HERMES_PID" -o command=)
printf '%s\n' "$HERMES_COMMAND" | grep -F '/SophoNote.app/Contents/Resources/hermes/' >/dev/null
kill "$PID"
wait "$PID" || true
sleep 2
kill -0 "$HERMES_PID" 2>/dev/null && { echo "Hermes child leaked after host exit" >&2; exit 1; }
echo "isolated HOME smoke passed; this does not replace a separate clean macOS VM Gatekeeper test"
