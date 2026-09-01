#!/bin/sh
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

test "$(uname -s)" = Darwin || {
  echo "pnpm pack:macos must run on macOS (Apple Silicon for the primary release target)" >&2
  exit 2
}

case "$(uname -m)" in
  arm64) TARGET=aarch64-apple-darwin ;;
  x86_64) TARGET=x86_64-apple-darwin ;;
  *) echo "unsupported macOS architecture: $(uname -m)" >&2; exit 2 ;;
esac

if [ -n "${SOPHONOTE_PACK_TARGET:-}" ] && [ "$SOPHONOTE_PACK_TARGET" != "$TARGET" ]; then
  echo "Hermes CPython cannot be cross-compiled: SOPHONOTE_PACK_TARGET=$SOPHONOTE_PACK_TARGET host=$TARGET" >&2
  exit 2
fi

export HERMES_TARGET="$TARGET"
cd "$ROOT"

PINNED_COMMIT=$(sed -n 's/^COMMIT=//p' "$SCRIPT_DIR/build-hermes-sidecar.sh" | head -1)
PINNED_VERSION=$(sed -n 's/^VERSION=//p' "$SCRIPT_DIR/build-hermes-sidecar.sh" | head -1)
MANIFEST="$ROOT/src-tauri/resources/hermes/$TARGET/MANIFEST.toml"
if [ -n "${HERMES_SOURCE_DIR:-}" ]; then
  pnpm hermes:bundle
elif [ -f "$MANIFEST" ] \
  && grep -F "hermes_version = \"$PINNED_VERSION\"" "$MANIFEST" >/dev/null \
  && grep -F "hermes_commit = \"$PINNED_COMMIT\"" "$MANIFEST" >/dev/null \
  && grep -F "target = \"$TARGET\"" "$MANIFEST" >/dev/null \
  && test -e "$ROOT/src-tauri/resources/hermes/$TARGET/runtime/python/bin/python3"; then
  echo "reusing pinned Hermes sidecar $PINNED_VERSION/$PINNED_COMMIT at $TARGET (set HERMES_SOURCE_DIR to rebuild)"
else
  echo "Set HERMES_SOURCE_DIR to rebuild the sidecar, or keep a matching $TARGET bundle at src-tauri/resources/hermes/" >&2
  exit 2
fi

if find "$ROOT/src-tauri/resources/hermes/$TARGET/runtime/python" \
  -type f -name '_sysconfigdata_*.py' -exec grep -IlE '/Users/[^/]+/' {} + \
  | grep -q .
then
  echo "Hermes CPython metadata contains a maintainer home path; rebuild it with HERMES_SOURCE_DIR before packaging." >&2
  exit 2
fi

pnpm tauri build --bundles app

APP="$ROOT/src-tauri/target/release/bundle.noindex/macos/SophoNote.app"
test -d "$APP" || { echo "SophoNote.app missing after pack: $APP" >&2; exit 2; }

# Tauri CLI 2.9.x embeds create-dmg 1.2.1, whose detach retry loop exits
# before its first hdiutil call under Bash `set -e`. Build the unsigned
# preview image with the same deterministic hdiutil path used by the signed
# release pipeline instead of leaving a mounted rw.* image behind.
codesign --force --deep --sign - --timestamp=none "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

VERSION=$(sed -n 's/^[[:space:]]*"version": "\([^"]*\)".*/\1/p' "$ROOT/src-tauri/tauri.conf.json" | head -1)
test -n "$VERSION" || { echo "cannot read app version from tauri.conf.json" >&2; exit 2; }
case "$TARGET" in
  aarch64-apple-darwin) DMG_ARCH=aarch64 ;;
  x86_64-apple-darwin) DMG_ARCH=x64 ;;
  *) echo "unsupported DMG target: $TARGET" >&2; exit 2 ;;
esac
DMG_DIR="$ROOT/src-tauri/target/release/bundle/dmg"
DMG="$DMG_DIR/SophoNote_${VERSION}_${DMG_ARCH}.dmg"
mkdir -p "$DMG_DIR"
test ! -e "$DMG" || find "$DMG" -delete

DMG_STAGE=$(mktemp -d "${TMPDIR:-/tmp}/sophonote-unsigned-dmg.XXXXXX")
cleanup_stage() {
  test ! -d "$DMG_STAGE" || find "$DMG_STAGE" -depth -delete
}
trap cleanup_stage EXIT INT TERM
ditto "$APP" "$DMG_STAGE/SophoNote.app"
ln -s /Applications "$DMG_STAGE/Applications"
hdiutil create -volname SophoNote -srcfolder "$DMG_STAGE" -ov -format UDZO "$DMG"
hdiutil verify "$DMG"

EVIDENCE="$ROOT/src-tauri/target/release/pack-evidence"
mkdir -p "$EVIDENCE"
{
  printf 'kind=unsigned-pack\n'
  printf 'target=%s\n' "$TARGET"
  printf 'app=%s\n' "$APP"
  printf 'dmg=%s\n' "$DMG"
  printf 'not_an_rc=1\n'
} > "$EVIDENCE/macos.txt"
echo "unsigned Apple pack (not an RC): $DMG"
echo "app: $APP"
