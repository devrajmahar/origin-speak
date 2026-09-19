# Origin Speak Setup

Origin Speak is a local voice-to-text product with two native Rust executables: the `origin` CLI manager and the silent resident `origin-runtime`. The runtime captures microphone audio, transcribes with local Whisper or Canary-Qwen, delivers text to the focused application, and shows only a compact status overlay. Configuration and model management belong to the CLI.

No Electron/React/Node.js runtime, browser/WebView, local HTTP server, cloud transcription service, dashboard, settings window, or assistant mode is required.

## Prerequisites

Install Rust stable and the platform compiler toolchain.

- Windows: Visual Studio Build Tools with C++. The Vulkan SDK is required only for `gpu-vulkan` builds.
- macOS: macOS 13+, Xcode Command Line Tools, Microphone permission, and Accessibility permission for focused-app text delivery.

## Build from source

```bash
cargo build --manifest-path native/Cargo.toml --bin origin --bin origin-runtime
```

GPU builds:

```bash
# Windows
cargo build --manifest-path native/Cargo.toml --features gpu-vulkan --bin origin --bin origin-runtime

# macOS
cargo build --manifest-path native/Cargo.toml --features gpu-metal --bin origin --bin origin-runtime
```

> Windows Vulkan note: nested whisper.cpp/CMake builds can exceed MSVC path limits. If compilation fails with `error C1083: Cannot open compiler generated file: ''`, use a short target directory, for example `$env:CARGO_TARGET_DIR = "C:\ltarget"`.

## Configure with `origin`

Start with:

```text
origin setup
```

Useful management commands include:

```text
origin status
origin doctor
origin model list
origin model status
origin model download <id>
origin model select <id>
origin model remove <id>
origin mic list
origin mic status
origin mic select <name>
origin hotkey show
origin hotkey set <chord>
origin config list
origin config get <key>
origin config set <key> <value>
origin config reset <key>
origin autostart status
origin start
origin stop
origin restart
origin update
origin update check
origin upgrade
origin uninstall
```

The manager supports `--json` for machine-readable output without ANSI/progress animation. In a human terminal, model and update downloads show live byte/percentage/throughput/ETA progress.

On Windows, `origin update` may report that installation is scheduled because the running `origin.exe` cannot replace itself. The post-exit helper records the final result durably; the next `origin status` or update command reports whether that replacement completed or failed.

Local transcription needs no API key. Origin Speak supports its whisper.cpp catalog plus the English-only `canary-qwen-2.5b` model through the native transcribe.cpp runtime. Model downloads are pinned to immutable upstream revisions and verified with exact artifact metadata and SHA-256 before installation. Safe interrupted downloads retain a valid `.part` file and resume with HTTP Range when the server supports it. Resume starts from the persisted byte count without re-hashing an incomplete multi-gigabyte partial first; once transfer completes, the CLI switches to an explicit SHA-256 verification phase.

## Default dictation hotkey

- Windows: `Shift+Space`
- macOS: `Shift+Space`

There is one global shortcut only. Hold it to capture speech and release it to transcribe and type the result. Change it with `origin hotkey set <chord>`; a running resident runtime is restarted automatically so the new binding takes effect.

The resident overlay exposes only four transient states: `Listening`, `Processing`, `Success`, and `Error`.

## Runtime lifecycle

`origin-runtime` is the silent resident process. Use the manager for lifecycle operations:

```text
origin start
origin status
origin restart
origin stop
```

Windows lifecycle control uses native process/IPC primitives and starts the runtime without a console window. macOS lifecycle support must continue to use native single-instance/quit mechanisms; do not substitute `pkill`, `osascript`, shell scripts, or process-name guessing.

## Uninstall behavior

By default:

```text
origin uninstall
```

removes the installed manager/runtime plus app-owned models, configuration, dictionary/database state, recognized legacy ListenOS model roots/update payloads, and stale Origin Speak update payloads. Exact product-owned roots are validated and symlinks/reparse points are not traversed.

Use `--keep-data` only when intentionally retaining user data. Destructive non-interactive uninstall requires `--yes`.

## Validate

```bash
cargo fmt --manifest-path backend/Cargo.toml -- --check
cargo test --manifest-path backend/Cargo.toml --locked
cargo fmt --manifest-path native/Cargo.toml -- --check
cargo check --manifest-path native/Cargo.toml --locked
```

## Release packaging

The release pipeline builds `origin` and `origin-runtime` as separate binaries and publishes them with the `origin-speak` artifact slug on GitHub Releases. Windows signing is applied when its complete signing input set is configured; otherwise unsigned binaries are published with that trust status in the release notes. macOS uses Developer ID signing/notarization when the full Apple credential set is configured and ad-hoc signing otherwise. The product no longer depends on NSIS, DMG, or Cloudflare R2 distribution.

Native Linux packaging is currently unsupported because the passive GPUI overlay cannot yet guarantee safe behavior across both Wayland and X11, and the supported install/autostart lifecycle has not been completed there.
