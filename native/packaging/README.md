# Native release packaging

This directory contains the shipping release packaging path for the GPUI application. Tagged releases build and publish native Windows and macOS artifacts only.

The packaging entry points expect the native binary to have already been built with:

```text
cargo build --manifest-path native/Cargo.toml --release --locked
```

## Windows

`windows/package.ps1` wraps the release binary in a per-user NSIS installer. It installs `ListenOS.exe` under `%LOCALAPPDATA%\Programs\ListenOS`, creates Start Menu and optional desktop shortcuts, and removes the ListenOS login Run-key value during uninstall so an uninstalled binary is not left as an autostart target.

The script requires `makensis` on `PATH`. CI installs NSIS before invoking it.

Example:

```text
powershell -NoProfile -File native/packaging/windows/package.ps1 -Version 0.1.21 -OutputDir dist/native/windows
```

Local packages are unsigned unless `WINDOWS_SIGNING_CERTIFICATE_PATH`, `WINDOWS_SIGNING_CERTIFICATE_PASSWORD`, and `WINDOWS_SIGN_TIMESTAMP_URL` are set. Tagged releases require those values through CI and sign both the application executable and the NSIS installer before publication.

## macOS

`macos/package.sh` creates `ListenOS.app`, signs it with the repository's current entitlements, verifies the bundle, and produces a compressed DMG plus a ZIP of the app bundle. Tagged releases build both Apple Silicon and Intel targets and merge them with `lipo`, so the published application and updater DMG are universal. The generated `Info.plist` contains the microphone and Apple Events usage descriptions required by the current application behavior.

Local packages use an ad-hoc signature unless `MACOS_CODESIGN_IDENTITY` names a Developer ID Application identity available in the current keychain. Setting `MACOS_NOTARIZE=1` additionally requires `APPLE_ID`, `APPLE_TEAM_ID`, and `APPLE_APP_SPECIFIC_PASSWORD`; the script notarizes the signed app, staples it, then notarizes and staples the DMG. Tagged releases require Developer ID signing and notarization before publication.

Example:

```text
bash native/packaging/macos/package.sh 0.1.21 dist/native/macos
```

The native macOS package currently declares macOS 13.0 as its minimum version. That matches the native shell's `SMAppService` login-item implementation and the supported native release baseline.

## Linux

There is no native Linux package in this directory. The current GPUI overlay path explicitly disables the passive status overlay on Linux because it cannot guarantee click-through behavior on both Wayland and X11. The native release workflow reports Linux packaging as unsupported instead of publishing a partial desktop artifact.

## Native updater manifest

`generate_update_manifest.py` emits `native-update.json`. The release workflow publishes the same manifest at `releases/v<version>/native-update.json` and at the R2 root as `native-update.json`. Artifact URLs inside the manifest always point at the immutable versioned release root.

Tagged Windows and macOS builds compile `LISTENOS_UPDATE_MANIFEST_URL` as `<CLOUDFLARE_R2_PUBLIC_BASE_URL>/native-update.json` before building the native binary. The workflow accepts only a credential-free HTTPS public base URL, and the publication job uses that same base to generate artifact URLs.

The schema is exact and versioned:

```json
{
  "schema_version": 1,
  "version": "0.1.21",
  "artifacts": {
    "windows": {
      "kind": "nsis-installer",
      "arch": "x86_64",
      "path": "ListenOS-0.1.21-Setup-x86_64.exe",
      "url": "https://updates.example/releases/v0.1.21/ListenOS-0.1.21-Setup-x86_64.exe",
      "sha256": "<lowercase hex sha256>"
    },
    "macos": {
      "kind": "dmg-installer",
      "arch": "universal",
      "path": "ListenOS-0.1.21-macos-universal.dmg",
      "url": "https://updates.example/releases/v0.1.21/ListenOS-0.1.21-macos-universal.dmg",
      "sha256": "<lowercase hex sha256>"
    }
  }
}
```

The shipping macOS updater artifact is always `arch: "universal"`, containing both `arm64` and `x86_64` slices. The native runtime accepts the DMG as the installable update package, opens it after SHA-256 verification, and accepts the Windows NSIS executable equivalently. `validate_update_manifest.py` checks the generated manifest, immutable artifact URLs, package names, architectures, and hashes against this shipping contract before publication.
