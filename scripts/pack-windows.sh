#!/bin/sh
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

is_windows_host() {
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*|Windows_NT) return 0 ;;
  esac
  [ "${OS:-}" = "Windows_NT" ]
}

is_windows_host || {
  echo "pnpm pack:windows must run on Windows x64 (Git Bash or CI bash). macOS cannot cross-compile NSIS/WebView2." >&2
  exit 2
}

export HERMES_TARGET=x86_64-pc-windows-msvc
cd "$ROOT"

PINNED_COMMIT=$(sed -n 's/^COMMIT=//p' "$SCRIPT_DIR/build-hermes-sidecar.sh" | head -1)
PINNED_VERSION=$(sed -n 's/^VERSION=//p' "$SCRIPT_DIR/build-hermes-sidecar.sh" | head -1)
MANIFEST="$ROOT/src-tauri/resources/hermes/$HERMES_TARGET/MANIFEST.toml"
if [ -n "${HERMES_SOURCE_DIR:-}" ]; then
  pnpm hermes:bundle
elif [ -f "$MANIFEST" ] \
  && grep -F "hermes_version = \"$PINNED_VERSION\"" "$MANIFEST" >/dev/null \
  && grep -F "hermes_commit = \"$PINNED_COMMIT\"" "$MANIFEST" >/dev/null \
  && grep -F "target = \"$HERMES_TARGET\"" "$MANIFEST" >/dev/null \
  && { test -e "$ROOT/src-tauri/resources/hermes/$HERMES_TARGET/runtime/python/python.exe" \
    || test -e "$ROOT/src-tauri/resources/hermes/$HERMES_TARGET/runtime/python/bin/python.exe"; }; then
  echo "reusing pinned Hermes sidecar $PINNED_VERSION/$PINNED_COMMIT at $HERMES_TARGET (set HERMES_SOURCE_DIR to rebuild)"
else
  echo "Set HERMES_SOURCE_DIR to rebuild the sidecar, or keep a matching $HERMES_TARGET bundle at src-tauri/resources/hermes/" >&2
  exit 2
fi

pnpm tauri build --bundles nsis

NSIS=$(find "$ROOT/src-tauri/target/release/bundle/nsis" -maxdepth 1 \( -name '*setup.exe' -o -name '*.exe' \) -print 2>/dev/null | head -1 || true)
test -n "$NSIS" && test -f "$NSIS" || { echo "NSIS installer missing after pack" >&2; exit 2; }

EVIDENCE="$ROOT/src-tauri/target/release/pack-evidence"
mkdir -p "$EVIDENCE"
{
  printf 'kind=unsigned-pack\n'
  printf 'target=x86_64-pc-windows-msvc\n'
  printf 'nsis=%s\n' "$NSIS"
  printf 'not_an_rc=1\n'
} > "$EVIDENCE/windows.txt"
echo "unsigned Windows pack (not an RC): $NSIS"
