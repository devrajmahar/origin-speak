#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "Usage: $0 <version> [output-dir]" >&2
  exit 2
fi

VERSION="$1"
BUNDLE_VERSION="${VERSION%%[-+]*}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUTPUT_DIR="${2:-$PROJECT_ROOT/dist/native/macos}"
BINARY="$PROJECT_ROOT/native/target/release/listenos-native"
INFO_TEMPLATE="$SCRIPT_DIR/Info.plist"
ENTITLEMENTS="$PROJECT_ROOT/backend/entitlements.plist"
ICON="$PROJECT_ROOT/native/assets/app-icon.icns"
ARCHS="$(lipo -archs "$BINARY" 2>/dev/null || true)"
if [[ "$ARCHS" == *arm64* && "$ARCHS" == *x86_64* ]]; then
  ARCH="universal"
elif [[ "$ARCHS" == "arm64" || "$ARCHS" == "x86_64" ]]; then
  ARCH="$ARCHS"
else
  echo "Could not determine supported macOS architecture(s) for $BINARY: ${ARCHS:-unknown}" >&2
  exit 1
fi
APP="$OUTPUT_DIR/ListenOS.app"
DMG="$OUTPUT_DIR/ListenOS-$VERSION-macos-$ARCH.dmg"
ZIP="$OUTPUT_DIR/ListenOS-$VERSION-macos-$ARCH.zip"
DMG_STAGE="$OUTPUT_DIR/.dmg-stage"

for required in "$BINARY" "$INFO_TEMPLATE" "$ENTITLEMENTS" "$ICON"; do
  if [[ ! -f "$required" ]]; then
    echo "Required packaging input is missing: $required" >&2
    exit 1
  fi
done

rm -rf "$APP" "$DMG_STAGE"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$DMG_STAGE"

cp "$BINARY" "$APP/Contents/MacOS/ListenOS"
chmod 755 "$APP/Contents/MacOS/ListenOS"
cp "$ICON" "$APP/Contents/Resources/AppIcon.icns"
cp "$INFO_TEMPLATE" "$APP/Contents/Info.plist"

plutil -replace CFBundleShortVersionString -string "$BUNDLE_VERSION" "$APP/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$BUNDLE_VERSION" "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"

SIGN_IDENTITY="${MACOS_CODESIGN_IDENTITY:--}"
NOTARIZE="${MACOS_NOTARIZE:-0}"
if [[ "$SIGN_IDENTITY" == "-" ]]; then
  if [[ "$NOTARIZE" == "1" ]]; then
    echo "MACOS_NOTARIZE=1 requires a Developer ID identity in MACOS_CODESIGN_IDENTITY." >&2
    exit 1
  fi
  codesign \
    --force \
    --options runtime \
    --timestamp=none \
    --entitlements "$ENTITLEMENTS" \
    --sign - \
    "$APP"
else
  codesign \
    --force \
    --options runtime \
    --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGN_IDENTITY" \
    "$APP"
fi

codesign --verify --deep --strict --verbose=2 "$APP"
codesign -d --entitlements :- "$APP" >/dev/null

if [[ "$NOTARIZE" == "1" ]]; then
  for variable in APPLE_ID APPLE_TEAM_ID APPLE_APP_SPECIFIC_PASSWORD; do
    if [[ -z "${!variable:-}" ]]; then
      echo "MACOS_NOTARIZE=1 requires $variable." >&2
      exit 1
    fi
  done

  NOTARY_ZIP="$OUTPUT_DIR/.ListenOS-notarization.zip"
  rm -f "$NOTARY_ZIP"
  ditto -c -k --sequesterRsrc --keepParent "$APP" "$NOTARY_ZIP"
  xcrun notarytool submit "$NOTARY_ZIP" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --wait
  rm -f "$NOTARY_ZIP"
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP"
fi

cp -R "$APP" "$DMG_STAGE/ListenOS.app"
ln -s /Applications "$DMG_STAGE/Applications"

rm -f "$DMG" "$ZIP"
hdiutil create \
  -volname "ListenOS" \
  -srcfolder "$DMG_STAGE" \
  -ov \
  -format UDZO \
  "$DMG"
hdiutil verify "$DMG"

if [[ "$NOTARIZE" == "1" ]]; then
  xcrun notarytool submit "$DMG" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --wait
  xcrun stapler staple "$DMG"
  xcrun stapler validate "$DMG"
fi

ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
rm -rf "$DMG_STAGE"

(
  cd "$OUTPUT_DIR"
  shasum -a 256 "$(basename "$DMG")" "$(basename "$ZIP")" > SHA256SUMS.txt
)

echo "Created $APP"
echo "Created $DMG"
echo "Created $ZIP"
