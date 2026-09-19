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
MANAGER_BINARY="${ORIGIN_SPEAK_MANAGER_BINARY:-$PROJECT_ROOT/native/target/release/origin}"
RUNTIME_BINARY="${ORIGIN_SPEAK_RUNTIME_BINARY:-$PROJECT_ROOT/native/target/release/origin-runtime}"
INFO_TEMPLATE="$SCRIPT_DIR/Info.plist"
ENTITLEMENTS="$PROJECT_ROOT/backend/entitlements.plist"
ICON="$PROJECT_ROOT/native/assets/app-icon.icns"
BUNDLE_IDENTIFIER="com.originspeak.app"
BUNDLE_EXECUTABLE="origin-runtime"

for required in "$MANAGER_BINARY" "$RUNTIME_BINARY" "$INFO_TEMPLATE" "$ENTITLEMENTS" "$ICON"; do
  if [[ ! -f "$required" ]]; then
    echo "Required packaging input is missing: $required" >&2
    exit 1
  fi
done

MANAGER_ARCHS="$(lipo -archs "$MANAGER_BINARY" 2>/dev/null || true)"
RUNTIME_ARCHS="$(lipo -archs "$RUNTIME_BINARY" 2>/dev/null || true)"
if [[ "$MANAGER_ARCHS" == *arm64* && "$MANAGER_ARCHS" == *x86_64* && "$RUNTIME_ARCHS" == *arm64* && "$RUNTIME_ARCHS" == *x86_64* ]]; then
  ARCH="universal"
elif [[ "$MANAGER_ARCHS" == "$RUNTIME_ARCHS" && ( "$MANAGER_ARCHS" == "arm64" || "$MANAGER_ARCHS" == "x86_64" ) ]]; then
  ARCH="$MANAGER_ARCHS"
else
  echo "Manager/runtime architectures must match; manager=${MANAGER_ARCHS:-unknown}, runtime=${RUNTIME_ARCHS:-unknown}" >&2
  exit 1
fi

mkdir -p "$OUTPUT_DIR"
APP="$OUTPUT_DIR/Origin Speak.app"
MANAGER="$OUTPUT_DIR/origin-speak-$VERSION-macos-$ARCH"
RUNTIME_ZIP="$OUTPUT_DIR/origin-speak-runtime-$VERSION-macos-$ARCH.zip"
rm -rf "$APP"
rm -f "$MANAGER" "$RUNTIME_ZIP"

cp "$MANAGER_BINARY" "$MANAGER"
chmod 755 "$MANAGER"

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$RUNTIME_BINARY" "$APP/Contents/MacOS/origin-runtime"
chmod 755 "$APP/Contents/MacOS/origin-runtime"
cp "$ICON" "$APP/Contents/Resources/AppIcon.icns"
cp "$INFO_TEMPLATE" "$APP/Contents/Info.plist"
plutil -replace CFBundleShortVersionString -string "$BUNDLE_VERSION" "$APP/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$BUNDLE_VERSION" "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
[[ "$(plutil -extract CFBundleIdentifier raw -o - "$APP/Contents/Info.plist")" == "$BUNDLE_IDENTIFIER" ]]
[[ "$(plutil -extract CFBundleExecutable raw -o - "$APP/Contents/Info.plist")" == "$BUNDLE_EXECUTABLE" ]]
[[ "$(plutil -extract CFBundlePackageType raw -o - "$APP/Contents/Info.plist")" == "APPL" ]]

SIGN_IDENTITY="${MACOS_CODESIGN_IDENTITY:--}"
NOTARIZE="${MACOS_NOTARIZE:-0}"
if [[ "$SIGN_IDENTITY" == "-" ]]; then
  if [[ "$NOTARIZE" == "1" ]]; then
    echo "MACOS_NOTARIZE=1 requires a Developer ID identity in MACOS_CODESIGN_IDENTITY." >&2
    exit 1
  fi
  codesign --force --options runtime --timestamp=none --sign - "$MANAGER"
  codesign --force --options runtime --timestamp=none --entitlements "$ENTITLEMENTS" --sign - "$APP"
else
  codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$MANAGER"
  codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$SIGN_IDENTITY" "$APP"
fi

codesign --verify --strict --verbose=2 "$MANAGER"
codesign --verify --deep --strict --verbose=2 "$APP"
codesign -d --entitlements :- "$APP" >/dev/null

if [[ "$NOTARIZE" == "1" ]]; then
  for variable in APPLE_ID APPLE_TEAM_ID APPLE_APP_SPECIFIC_PASSWORD; do
    if [[ -z "${!variable:-}" ]]; then
      echo "MACOS_NOTARIZE=1 requires $variable." >&2
      exit 1
    fi
  done

  APP_NOTARY_ZIP="$OUTPUT_DIR/.origin-speak-runtime-notarization.zip"
  MANAGER_NOTARY_ZIP="$OUTPUT_DIR/.origin-speak-manager-notarization.zip"
  rm -f "$APP_NOTARY_ZIP" "$MANAGER_NOTARY_ZIP"
  ditto -c -k --sequesterRsrc --keepParent "$APP" "$APP_NOTARY_ZIP"
  ditto -c -k "$MANAGER" "$MANAGER_NOTARY_ZIP"
  xcrun notarytool submit "$APP_NOTARY_ZIP" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --wait
  xcrun notarytool submit "$MANAGER_NOTARY_ZIP" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --wait
  rm -f "$APP_NOTARY_ZIP" "$MANAGER_NOTARY_ZIP"
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP"
fi

ditto -c -k --sequesterRsrc --keepParent "$APP" "$RUNTIME_ZIP"

# Assert the published archive has exactly the layout expected by the CLI
# bootstrapper. This catches accidental flat-binary or nested-directory archives
# before they reach the update manifest.
VERIFY_ROOT="$OUTPUT_DIR/.runtime-zip-verify"
rm -rf "$VERIFY_ROOT"
mkdir -p "$VERIFY_ROOT"
ditto -x -k "$RUNTIME_ZIP" "$VERIFY_ROOT"
[[ -d "$VERIFY_ROOT/Origin Speak.app" ]]
[[ "$(find "$VERIFY_ROOT" -mindepth 1 -maxdepth 1 -print | wc -l | tr -d ' ')" == "1" ]]
[[ -x "$VERIFY_ROOT/Origin Speak.app/Contents/MacOS/origin-runtime" ]]
[[ "$(plutil -extract CFBundleIdentifier raw -o - "$VERIFY_ROOT/Origin Speak.app/Contents/Info.plist")" == "$BUNDLE_IDENTIFIER" ]]
codesign --verify --deep --strict --verbose=2 "$VERIFY_ROOT/Origin Speak.app"
rm -rf "$VERIFY_ROOT"
(
  cd "$OUTPUT_DIR"
  shasum -a 256 "$(basename "$MANAGER")" "$(basename "$RUNTIME_ZIP")" > SHA256SUMS-macos.txt
)

echo "Created $MANAGER"
echo "Created $APP"
echo "Created $RUNTIME_ZIP"
