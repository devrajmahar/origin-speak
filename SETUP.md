# ListenOS Setup

ListenOS is a native GPUI desktop application linked directly to its Rust voice engine. No browser runtime, separate local server, or cloud login is required.

## Install prerequisites

Install Rust stable and the platform compiler toolchain.

Windows requires Visual Studio Build Tools with C++. macOS requires Xcode Command Line Tools and macOS 13 or newer.

## Run from source

```bash
cargo run --manifest-path native/Cargo.toml
```

That plain build transcribes on CPU. For GPU-accelerated local Whisper, enable the platform backend (Windows requires the Vulkan SDK):

```bash
# Windows (requires the Vulkan SDK)
cargo run --manifest-path native/Cargo.toml --features gpu-vulkan
# macOS
cargo run --manifest-path native/Cargo.toml --features gpu-metal
```

Settings -> System reports the active backend and any CPU-fallback reason. On CPU-only builds, the `tiny.en` model has far lower latency than the default `base.en`.

> Windows note: the Vulkan Whisper build nests CMake projects several levels deep, and MSVC refuses object paths over ~250 characters. If the Vulkan build fails with `error C1083: Cannot open compiler generated file: ''`, point Cargo at a short target directory before building:
>
> ```powershell
> $env:CARGO_TARGET_DIR = "C:\ltarget"
> cargo build --manifest-path native/Cargo.toml --release --locked --features gpu-vulkan
> ```

Dictation uses local Whisper inference and does not require a transcription API key. During first-run setup, ListenOS downloads the selected model into the per-user models directory:

```text
<user data directory>/ListenOS/models
```

Model selection, download state, microphone selection, shortcuts, language preferences, and other settings are available inside the native Settings view.

## Optional runtime environment

Sensitive command confirmations can be enabled for all confirmation-capable actions by setting this environment variable before launching ListenOS:

```text
LISTENOS_REQUIRE_CONFIRMATION=true
```

Power actions remain confirmation-gated even when that variable is not enabled.

## Validate

```bash
cargo fmt --manifest-path backend/Cargo.toml -- --check
cargo test --manifest-path backend/Cargo.toml --locked
cargo fmt --manifest-path native/Cargo.toml -- --check
cargo check --manifest-path native/Cargo.toml --locked
```

## Package

Build the release binary first:

```bash
cargo build --manifest-path native/Cargo.toml --release --locked
```

Windows packaging uses `native/packaging/windows/package.ps1` and NSIS. macOS packaging uses `native/packaging/macos/package.sh`. Tagged releases perform the required signing/notarization and publish native update metadata through the release workflow.

Native Linux packaging is currently unsupported because the passive overlay cannot yet guarantee click-through behavior across both Wayland and X11.

## Default shortcuts

- Hold-to-talk: `Meta+Ctrl+Space` (Win+Ctrl+Space) on Windows/Linux, `Ctrl+Space` on macOS
- Assistant mode: `Ctrl+Alt+Space`

Both are configurable in `Settings -> General`.
