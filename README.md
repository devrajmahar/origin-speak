# ListenOS

AI-powered native desktop voice control for Windows and macOS.

ListenOS is a single-process Rust desktop application built with GPUI. The application owns its UI components on top of `gpui-base` and links the reusable Rust voice engine directly in-process through `voice_os_lib`.

![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS-blue) ![GPUI](https://img.shields.io/badge/GPUI-native-3e63dd) ![Rust](https://img.shields.io/badge/Rust-stable-red)

## Features

- Push-to-talk dictation and assistant mode with configurable global shortcuts
- Local Whisper transcription and model management
- Voice-to-action command execution with confirmation for sensitive actions
- Conversation history, clipboard tools, dictionary, snippets, custom commands, integrations, and style controls
- Native tray/menu-bar lifecycle, autostart, single-instance activation, and `listenos://` deep links
- Native update checks with verified release manifests and package hashes
- Local settings and persistence with no web runtime or separate backend process

## Supported desktop releases

### Windows

- Windows 10/11 (64-bit)
- Rust stable
- Visual Studio Build Tools with the C++ workload
- NSIS only when building an installer locally

### macOS

- macOS 13+
- Rust stable
- Xcode Command Line Tools
- Microphone and Accessibility permissions for voice capture and automation

Native Linux packaging is intentionally not shipped yet. The current GPUI path cannot guarantee safe click-through behavior for passive overlay surfaces on both Wayland and X11. Linux release support requires a native package/desktop entry, protocol and autostart integration, and a safe cross-backend overlay/input implementation.

## Quick start

```bash
cargo run --manifest-path native/Cargo.toml
```

That plain build is CPU-only for transcription. For GPU-accelerated local Whisper, build with the platform backend (Windows needs the Vulkan SDK installed):

```bash
# Windows (requires the Vulkan SDK)
cargo run --manifest-path native/Cargo.toml --features gpu-vulkan
# macOS
cargo run --manifest-path native/Cargo.toml --features gpu-metal
```

Settings -> System shows the active backend (`Active · Vulkan …` / `Active · Metal …` versus a CPU-fallback reason). If you are stuck on CPU, prefer the `tiny.en` model for much lower latency; `base.en` on CPU takes several seconds per utterance.

Dictation runs locally with Whisper and does not require an API key. During first-run model setup, ListenOS downloads the selected model into the per-user ListenOS models directory:

```text
<user data directory>/ListenOS/models
```

Models and microphones can be changed later under `Settings -> System` and `Settings -> General`.

## Build and validation

```bash
cargo fmt --manifest-path backend/Cargo.toml -- --check
cargo test --manifest-path backend/Cargo.toml --locked
cargo fmt --manifest-path native/Cargo.toml -- --check
cargo check --manifest-path native/Cargo.toml --locked
cargo build --manifest-path native/Cargo.toml --release --locked
```

Native release packaging lives under `native/packaging/`. Windows uses a per-user NSIS installer. macOS produces a signed application bundle, DMG, and ZIP; tagged releases require production signing credentials and notarization.

## Default shortcuts

| Action | Default | Behavior |
|---|---|---|
| Hold-to-talk | `Meta+Ctrl+Space` on Windows/Linux, `Ctrl+Space` on macOS | Hold to record, release to process |
| Assistant mode | `Ctrl+Alt+Space` | Toggle hands-free listening |

Both shortcuts are configurable under `Settings -> General`.

## Architecture

```text
global shortcuts / native windows / tray / updater
                         |
                         v
                     GPUI app
                         |
                         v
                  voice_os_lib
              /       |       \
           audio   whisper   state/delivery
```

Repository layout:

```text
ListenOS/
|-- native/                  # GPUI desktop application and platform shell
|   |-- src/                 # Native windows, state, runtime bridge, views
|   `-- packaging/           # Windows/macOS packaging and update metadata helpers
|-- backend/                 # Reusable Rust application/voice core (rlib)
|   `-- src/
|       |-- commands/        # Typed application operations
|       |-- shortcuts.rs     # Transport-independent global shortcuts
|       |-- audio/
|       `-- transcription/   # Local Whisper model management and inference
|-- scripts/                 # Small release-version helpers
`-- docs/
```

The desktop runtime is one Rust process. GPUI calls typed core APIs directly. CPU-heavy transcription, downloads, persistence, update I/O, and other blocking work run away from the GPUI render thread.

The UI layering is application-owned:

```text
ListenOS UI / components
          |
          v
      gpui-base
          |
          v
         GPUI
```

See [`docs/gpui-native-architecture.md`](docs/gpui-native-architecture.md) for architecture and release-readiness requirements.

## Releases

Tagged releases use `.github/workflows/release.yml` to build the native Windows and macOS applications. Windows release artifacts are code-signed and timestamped. macOS release artifacts require Developer ID signing and notarization. The workflow publishes release packages and `native-update.json` to Cloudflare R2 for the native updater.

Version changes use one Python standard-library helper:

```bash
python scripts/version.py bump 0.1.22
python scripts/version.py sync
```

## License

Proprietary software. See [LICENSE](LICENSE). Third-party attributions are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
