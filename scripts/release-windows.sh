#!/bin/sh
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
: "${WINDOWS_CERTIFICATE_THUMBPRINT:?WINDOWS_CERTIFICATE_THUMBPRINT is required for a signed Windows RC}"
: "${SOPHONOTE_UPDATER_PUBKEY:?SOPHONOTE_UPDATER_PUBKEY is required}"
: "${SOPHONOTE_UPDATER_ENDPOINT:?SOPHONOTE_UPDATER_ENDPOINT is required}"
: "${TAURI_SIGNING_PRIVATE_KEY:?TAURI_SIGNING_PRIVATE_KEY is required for updater artifacts}"

is_windows_host() {
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*|Windows_NT) return 0 ;;
  esac
  [ "${OS:-}" = "Windows_NT" ]
}

is_windows_host || {
  echo "pnpm release:windows must run on Windows x64" >&2
  exit 2
}

"$SCRIPT_DIR/prepare-release-config-windows.sh"
export HERMES_TARGET=x86_64-pc-windows-msvc
cd "$ROOT"
pnpm hermes:bundle
pnpm tauri build --bundles nsis --config src-tauri/tauri.release.windows.conf.json

NSIS=$(find "$ROOT/src-tauri/target/release/bundle/nsis" -maxdepth 1 \( -name '*setup.exe' -o -name '*.exe' \) -print 2>/dev/null | head -1 || true)
test -n "$NSIS" && test -f "$NSIS" || { echo "NSIS installer missing" >&2; exit 2; }
UPDATER_ARCHIVE=$(find "$ROOT/src-tauri/target/release/bundle/nsis" -maxdepth 1 \( -name '*.zip' -o -name '*.nsis.zip' \) -print 2>/dev/null | head -1 || true)
test -n "$UPDATER_ARCHIVE" && test -f "$UPDATER_ARCHIVE" || { echo "updater archive missing" >&2; exit 2; }

EVIDENCE="$ROOT/src-tauri/target/release/release-evidence"
mkdir -p "$EVIDENCE"
{
  printf 'kind=signed-rc\n'
  printf 'target=x86_64-pc-windows-msvc\n'
  printf 'nsis=%s\n' "$NSIS"
  printf 'updater=%s\n' "$UPDATER_ARCHIVE"
} > "$EVIDENCE/windows.txt"
echo "signed Windows RC: $NSIS"
echo "release evidence: $EVIDENCE"
