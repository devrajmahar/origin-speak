# Native CLI/bootstrap release packaging

Origin Speak now targets two release binaries instead of a traditional desktop installer:

- `origin`: console CLI/bootstrap manager for setup, diagnostics, configuration, lifecycle, update and uninstall.
- `origin-runtime`: silent resident voice process that owns shortcuts and the compact GPUI overlays.

The packaging scripts expect those binaries to exist in `native/target/release/`. `native/Cargo.toml` declares both targets explicitly with automatic binary discovery disabled.

## Windows

`windows/package.ps1` copies the two binaries directly and signs them when the Windows certificate/password/timestamp inputs are available. It does not build or launch NSIS. Release artifacts are:

```text
origin-speak-<version>-windows-x86_64.exe
origin-speak-runtime-<version>-windows-x86_64.exe
```

The manager installs the runtime into the per-user Origin Speak directory using Rust/native filesystem APIs. Runtime start/stop does not invoke `cmd.exe` or PowerShell. Local packages are unsigned unless `WINDOWS_SIGNING_CERTIFICATE_PATH`, `WINDOWS_SIGNING_CERTIFICATE_PASSWORD`, and `WINDOWS_SIGN_TIMESTAMP_URL` are set.

```text
powershell -NoProfile -File native/packaging/windows/package.ps1 -Version 0.1.30 -OutputDir dist/native/windows
```

The retired NSIS installer source is no longer part of the shipping tree. CLI uninstall contains exact cleanup for its historical registry keys, shortcuts, old executable, and `Uninstall.exe` so upgrades do not leave dead registrations behind.

## macOS

`macos/package.sh` signs the console manager independently and packages the silent runtime in `Origin Speak.app`. The runtime is an `LSUIElement` background app, requests microphone access only for local voice-to-text dictation, and does not advertise the retired legacy `listenos://` URL handler or obsolete Apple Events usage. Accessibility permission remains a runtime/TCC requirement for reliable text injection into other applications. Release artifacts are:

```text
origin-speak-<version>-macos-universal
origin-speak-runtime-<version>-macos-universal.zip
```

Tagged releases build Apple Silicon and Intel manager/runtime binaries and merge each with `lipo`. With a complete Apple credential set the workflow applies Developer ID signing, notarization, and stapling; without it the package script uses ad-hoc code signing and publishes a clearly marked non-notarized build. There is no DMG in the CLI/bootstrap contract.

```text
bash native/packaging/macos/package.sh 0.1.30 dist/native/macos
```

The runtime bundle keeps macOS 13.0 as its minimum version.

## Bootstrap update manifest

`generate_update_manifest.py` emits schema-v2 `bootstrap-update.json`. Each supported platform contains a separately hashed manager and runtime payload so the CLI can stage and verify both before replacement. Tagged releases publish this manifest, the manager/runtime payloads, platform checksum files, and trust-status sidecars on the canonical GitHub Release at `https://github.com/devrajmahar/origin-speak/releases/tag/v<version>`.

```json
{
  "schema_version": 2,
  "version": "0.1.30",
  "platforms": {
    "windows-x86_64": {
      "manager": {
        "kind": "cli-manager",
        "arch": "x86_64",
        "path": "origin-speak-0.1.30-windows-x86_64.exe",
        "url": "https://github.com/devrajmahar/origin-speak/releases/download/v0.1.30/origin-speak-0.1.30-windows-x86_64.exe",
        "sha256": "<lowercase hex sha256>"
      },
      "runtime": {
        "kind": "silent-runtime",
        "arch": "x86_64",
        "path": "origin-speak-runtime-0.1.30-windows-x86_64.exe",
        "url": "https://github.com/devrajmahar/origin-speak/releases/download/v0.1.30/origin-speak-runtime-0.1.30-windows-x86_64.exe",
        "sha256": "<lowercase hex sha256>"
      }
    },
    "macos-universal": {
      "manager": {
        "kind": "cli-manager",
        "arch": "universal",
        "path": "origin-speak-0.1.30-macos-universal",
        "url": "https://github.com/devrajmahar/origin-speak/releases/download/v0.1.30/origin-speak-0.1.30-macos-universal",
        "sha256": "<lowercase hex sha256>"
      },
      "runtime": {
        "kind": "app-bundle-zip",
        "arch": "universal",
        "path": "origin-speak-runtime-0.1.30-macos-universal.zip",
        "url": "https://github.com/devrajmahar/origin-speak/releases/download/v0.1.30/origin-speak-runtime-0.1.30-macos-universal.zip",
        "sha256": "<lowercase hex sha256>"
      }
    }
  }
}
```

`validate_update_manifest.py` verifies the exact filenames, kinds, architectures, immutable URLs, and SHA-256 values before publication. `test_update_manifest.py` covers round-trip generation and tamper rejection.

## Linux

No Linux release artifact is published yet. The current passive GPUI overlay cannot guarantee safe click-through behavior across both Wayland and X11, so the CLI must report Linux runtime installation as unsupported instead of implying parity that does not exist.
