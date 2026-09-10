#!/bin/sh
# Reproducible macOS embedded component. No global installation or TCC changes.
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
test "$(uname -s)" = Darwin || exit 0
VERSION=0.26.0
SHA256=dccfb8f06a5b22a976a2391ca994d8c9ed8562a2cb16e779ee5428204666f265
CACHE="$ROOT/src-tauri/target/computer-use-download"
ARCHIVE="$CACHE/cua-driver-$VERSION.tar.gz"
OUTPUT="$ROOT/src-tauri/resources/computer-use"
mkdir -p "$CACHE" "$OUTPUT"
verify_archive() {
  test -f "$ARCHIVE" && test "$(shasum -a 256 "$ARCHIVE" | cut -d ' ' -f1)" = "$SHA256"
}
if ! verify_archive; then
  # Partial downloads remain resumable; a completed hash mismatch is rejected.
  curl -fL --connect-timeout 15 --max-time 300 --retry 2 -C - \
    "https://github.com/trycua/cua/releases/download/cua-driver-rs-v$VERSION/cua-driver-rs-$VERSION-darwin-universal-binary.tar.gz" \
    -o "$ARCHIVE"
  verify_archive || { echo 'Computer-use archive SHA-256 mismatch; remove the failed cached archive and retry' >&2; exit 2; }
fi
# Extract only the executable; no standalone .app, SDKs, or global symlinks.
tar -xzf "$ARCHIVE" -C "$OUTPUT" cua-driver
cp "$SCRIPT_DIR/assets/hermes-cua" "$OUTPUT/hermes-cua"
cp "$SCRIPT_DIR/assets/LICENSE.cua" "$OUTPUT/LICENSE.cua"
chmod 755 "$OUTPUT/cua-driver" "$OUTPUT/hermes-cua"
if test -n "${APPLE_SIGNING_IDENTITY:-}"; then
  codesign --force --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$OUTPUT/cua-driver"
else
  codesign --force --sign - --timestamp=none "$OUTPUT/cua-driver"
fi
codesign --verify --strict "$OUTPUT/cua-driver"
printf 'Embedded computer-use component %s prepared\n' "$VERSION"
