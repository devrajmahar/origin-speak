# Origin Speak

Open-source, fully native local voice-to-text for Windows and macOS.

Origin Speak is split into two native Rust programs:

- `origin`: the console manager for setup, models, microphones, configuration, lifecycle, updates, and uninstall.
- `origin-runtime`: the silent resident GPUI process that owns the global dictation hotkey, microphone capture, local Whisper transcription, text delivery, and compact status overlay.

There is no Electron, React, Node.js, browser/WebView runtime, HTTP bridge, JSON-RPC bridge, cloud transcription service, or assistant/action layer in the product architecture.

![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS-blue) ![GPUI](https://img.shields.io/badge/GPUI-native-3e63dd) ![Rust](https://img.shields.io/badge/Rust-stable-red)

## Features

- One configurable hold-to-talk dictation hotkey
- Local Whisper transcription with CPU, Vulkan (Windows), or Metal (macOS) execution
- Verified local model downloads pinned to immutable revisions and SHA-256 digests
- Recognition dictionary hints for names and specialized vocabulary
- Reliable focused-application text delivery with clipboard recovery when direct delivery fails
- Silent resident runtime with compact `Listening`, `Processing`, `Success`, and `Error` overlay states
- CLI-managed model, microphone, hotkey, autostart, update, runtime, and configuration workflows
- Safe uninstall that removes app-owned models, config, databases, runtime state, and installed binaries by default; `--keep-data` is the explicit opt-out

## Supported desktop releases

### Windows

- Windows 10/11 (64-bit)
- Rust stable
- Visual Studio Build Tools with the C++ workload
- Vulkan SDK only when building the Vulkan Whisper backend

### macOS

- macOS 13+
- Rust stable
- Xcode Command Line Tools
- Microphone permission for capture
- Accessibility permission for text injection into other applications

Native Linux packaging is intentionally not shipped yet. The current GPUI overlay path cannot guarantee safe passive-overlay behavior across both Wayland and X11. Linux release support also needs a supported native install/autostart lifecycle.

## Run from source

Build the manager and resident runtime separately:

```bash
cargo build --manifest-path native/Cargo.toml --bin origin --bin origin-runtime
```

For GPU-accelerated local Whisper:

```bash
# Windows (requires the Vulkan SDK)
cargo build --manifest-path native/Cargo.toml --features gpu-vulkan --bin origin --bin origin-runtime

# macOS
cargo build --manifest-path native/Cargo.toml --features gpu-metal --bin origin --bin origin-runtime
```

The plain build uses CPU transcription. `tiny.en` is the lowest-latency English model on CPU-constrained machines; `base.en` provides a larger default model when the machine has sufficient headroom.

## CLI setup

The manager is the configuration surface. Typical commands are:

```text
origin setup
origin status
origin doctor
origin model list
origin model download tiny.en
origin model select tiny.en
origin mic list
origin mic test
origin hotkey show
origin hotkey set Ctrl+Space
origin config list
origin autostart status
origin start
origin stop
origin restart
origin update check
origin uninstall
```

`origin setup` is idempotent: it keeps an already-valid model and matching configuration, downloads a model only when needed, installs the resident runtime, and can configure microphone/autostart choices.

Dictation uses local Whisper and does not require an API key. Models live under app-owned per-user Origin Speak model roots. Current installs prefer the local-data root; legacy ListenOS roots are discovered only for migration/cleanup compatibility. `origin uninstall` removes both current and recognized legacy model roots by default without deleting their parent data directories.

## Default shortcut and overlay

| Action | Default | Behavior |
|---|---|---|
| Dictation | `Meta+Ctrl+Space` on Windows/Linux development, `Ctrl+Space` on macOS | Hold to record; release to transcribe and deliver text |

Change it with `origin hotkey set <chord>` and restart/reload the resident runtime so the global registration is refreshed.

The resident UI is intentionally minimal. There is no dashboard or settings window. A compact non-activating overlay reports only `Listening`, `Processing`, `Success`, and `Error`; it must never steal focus from the application receiving dictated text.

## Architecture

```text
origin CLI manager
   | setup/config/models/mic/update/lifecycle/uninstall
   v
app-owned config + model/data roots

origin-runtime
   |
   +-- global dictation hotkey
   +-- CPAL microphone capture
   +-- local Whisper inference
   +-- focused-app text delivery
   `-- compact GPUI status overlay
```

Repository layout:

```text
origin-speak/
|-- native/
|   |-- src/                 # CLI manager, resident runtime, platform lifecycle
|   `-- packaging/           # Separate manager/runtime release artifact helpers
|-- backend/                 # Reusable local dictation engine (rlib)
|   `-- src/                 # audio, streaming, transcription, delivery, dictionary
|-- scripts/                 # Release/version helpers
`-- docs/
```

The runtime calls the Rust core directly through typed in-process APIs. CPU/GPU-heavy inference, downloads, persistence, and other blocking work stay off the GPUI render thread.

See [`docs/gpui-native-architecture.md`](docs/gpui-native-architecture.md) and [`docs/cli-management.md`](docs/cli-management.md) for implementation details.

## Build and validation

```bash
cargo fmt --manifest-path backend/Cargo.toml -- --check
cargo test --manifest-path backend/Cargo.toml --locked
cargo fmt --manifest-path native/Cargo.toml -- --check
cargo check --manifest-path native/Cargo.toml --locked
```

## Releases

Tagged releases publish the manager/runtime payloads, `bootstrap-update.json`, checksum files, and trust-status metadata on GitHub Releases at `devrajmahar/origin-speak`. Windows artifacts are signed and timestamped when the repository signing inputs are available, otherwise the release marks them unsigned. macOS artifacts use Developer ID signing and notarization when the full Apple credential set is available, otherwise the release publishes ad-hoc signed, non-notarized artifacts and says so in the release notes.

Version changes use the standard-library helper:

```bash
python scripts/version.py bump 0.1.22
python scripts/version.py sync
```

## License

Origin Speak is open-source software released under the [MIT License](LICENSE). Third-party attributions are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
