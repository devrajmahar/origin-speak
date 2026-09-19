#!/bin/sh
set -eu

MANIFEST_URL='https://github.com/devrajmahar/origin-speak/releases/latest/download/bootstrap-update.json'
STAGING="$(mktemp -d "${TMPDIR:-/tmp}/origin-speak-install.XXXXXX")"

cleanup() {
  rm -rf "$STAGING"
}
trap cleanup EXIT HUP INT TERM

download_verified() {
  url="$1"
  path="$2"
  expected="$3"

  curl -fL --retry 3 --connect-timeout 15 "$url" -o "$path"
  actual="$(/usr/bin/shasum -a 256 "$path" | awk '{print $1}')"
  if [ "$actual" != "$expected" ]; then
    echo "SHA-256 verification failed for $(basename "$path")" >&2
    exit 1
  fi
}

if [ "$(uname -s)" != 'Darwin' ]; then
  echo 'Origin Speak currently ships terminal installation for Windows and macOS only.' >&2
  exit 1
fi

echo 'Fetching latest Origin Speak release metadata...'
MANIFEST="$STAGING/bootstrap-update.json"
curl -fsSL --retry 3 --connect-timeout 15 "$MANIFEST_URL" -o "$MANIFEST"

VERSION="$(/usr/bin/plutil -extract version raw -o - "$MANIFEST")"
MANAGER_PATH="$(/usr/bin/plutil -extract platforms.macos-universal.manager.path raw -o - "$MANIFEST")"
MANAGER_URL="$(/usr/bin/plutil -extract platforms.macos-universal.manager.url raw -o - "$MANIFEST")"
MANAGER_SHA="$(/usr/bin/plutil -extract platforms.macos-universal.manager.sha256 raw -o - "$MANIFEST")"
RUNTIME_PATH="$(/usr/bin/plutil -extract platforms.macos-universal.runtime.path raw -o - "$MANIFEST")"
RUNTIME_URL="$(/usr/bin/plutil -extract platforms.macos-universal.runtime.url raw -o - "$MANIFEST")"
RUNTIME_SHA="$(/usr/bin/plutil -extract platforms.macos-universal.runtime.sha256 raw -o - "$MANIFEST")"

MANAGER="$STAGING/$MANAGER_PATH"
RUNTIME="$STAGING/$RUNTIME_PATH"

echo "Downloading Origin Speak $VERSION CLI..."
download_verified "$MANAGER_URL" "$MANAGER" "$MANAGER_SHA"
download_verified "$RUNTIME_URL" "$RUNTIME" "$RUNTIME_SHA"
chmod +x "$MANAGER"

echo 'Checksums verified. Starting terminal setup...'
"$MANAGER" setup

echo
echo 'Origin Speak is installed. Open a new terminal and run: origin status'
