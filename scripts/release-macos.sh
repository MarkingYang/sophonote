#!/bin/sh
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
: "${APPLE_SIGNING_IDENTITY:?APPLE_SIGNING_IDENTITY is required}"
: "${APPLE_NOTARY_PROFILE:?APPLE_NOTARY_PROFILE is required}"

"$SCRIPT_DIR/prepare-tauri-target.sh"
"$SCRIPT_DIR/prepare-release-config.sh"
cd "$ROOT"
pnpm hermes:bundle
# 只让 Tauri 生成并签名 App/updater；最终 DMG 在 App staple 后由下方
# hdiutil 路径重建，避开内置 create-dmg 的卸载缺陷，也保证镜像内是
# 已带离线 Gatekeeper ticket 的同一份 App。
pnpm tauri build --config src-tauri/tauri.release.conf.json --bundles app
APP="$ROOT/src-tauri/target/release/bundle.noindex/macos/SophoNote.app"
VERSION=$(sed -n 's/^[[:space:]]*"version": "\([^"]*\)".*/\1/p' "$ROOT/src-tauri/tauri.conf.json" | head -1)
test -n "$VERSION" || { echo "cannot read app version from tauri.conf.json" >&2; exit 2; }
case "$(uname -m)" in
  arm64) DMG_ARCH=aarch64 ;;
  x86_64) DMG_ARCH=x64 ;;
  *) echo "unsupported macOS architecture: $(uname -m)" >&2; exit 2 ;;
esac
DMG="$ROOT/src-tauri/target/release/bundle/dmg/SophoNote_${VERSION}_${DMG_ARCH}.dmg"
mkdir -p "$(dirname "$DMG")"
test -d "$APP"
test "$(basename "$APP")" = "SophoNote.app"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")" = "com.fei.sophonote"
UPDATER_ARCHIVE=$(find "$ROOT/src-tauri/target/release/bundle/macos" -maxdepth 1 -name '*.app.tar.gz' -print | head -1)
test -f "$UPDATER_ARCHIVE" || { echo "updater archive missing" >&2; exit 2; }
EVIDENCE="$ROOT/src-tauri/target/release/release-evidence"
mkdir -p "$EVIDENCE"

# build-hermes-sidecar signs nested Mach-O files before hashing; Tauri then
# signs the outer app and DMG. Nothing inside the app is mutated after this.
codesign --verify --deep --strict --verbose=2 "$APP"
codesign -d --verbose=4 "$APP" > "$EVIDENCE/app-codesign.txt" 2>&1

# Notarize/staple the standalone app first. Rebuild the DMG afterwards so the
# app inside the distributed image also contains its ticket, then sign,
# notarize and staple that final image.
ZIP="$ROOT/src-tauri/target/release/bundle/macos/SophoNote-notary.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
xcrun notarytool submit "$ZIP" --keychain-profile "$APPLE_NOTARY_PROFILE" \
  --wait --output-format json > "$EVIDENCE/app-notary.json"
cat "$EVIDENCE/app-notary.json"
test "$(plutil -extract status raw "$EVIDENCE/app-notary.json")" = "Accepted" || {
  echo "Apple rejected app notarization" >&2; exit 2;
}
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"

# Tauri creates the updater archive before notarization. Repack the stapled
# app and re-sign the final archive so an update installs the exact same app
# that ships in the DMG, including its offline Gatekeeper ticket.
rm -f "$UPDATER_ARCHIVE" "$UPDATER_ARCHIVE.sig"
COPYFILE_DISABLE=1 tar -czf "$UPDATER_ARCHIVE" -C "$(dirname "$APP")" "$(basename "$APP")"
pnpm tauri signer sign "$UPDATER_ARCHIVE"
test -s "$UPDATER_ARCHIVE.sig" || { echo "updater signature missing" >&2; exit 2; }

DMG_STAGE=$(mktemp -d "${TMPDIR:-/tmp}/sophonote-dmg.XXXXXX")
trap 'rm -rf "$DMG_STAGE"' EXIT INT TERM
ditto "$APP" "$DMG_STAGE/SophoNote.app"
ln -s /Applications "$DMG_STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname SophoNote -srcfolder "$DMG_STAGE" -ov -format UDZO "$DMG"
codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$DMG"
xcrun notarytool submit "$DMG" --keychain-profile "$APPLE_NOTARY_PROFILE" \
  --wait --output-format json > "$EVIDENCE/dmg-notary.json"
cat "$EVIDENCE/dmg-notary.json"
test "$(plutil -extract status raw "$EVIDENCE/dmg-notary.json")" = "Accepted" || {
  echo "Apple rejected DMG notarization" >&2; exit 2;
}
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG"
spctl --assess --type execute --verbose=4 "$APP" \
  > "$EVIDENCE/app-gatekeeper.txt" 2>&1
hdiutil verify "$DMG" > "$EVIDENCE/dmg-verify.txt" 2>&1
echo "signed, notarized and stapled: $DMG; signed updater: $UPDATER_ARCHIVE"
echo "release evidence: $EVIDENCE"
