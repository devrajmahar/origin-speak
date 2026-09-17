# ListenOS

AI-powered desktop voice control for Windows, macOS, and Linux.

ListenOS uses Electron for the desktop shell, a React interface bundled by Rspack, and a standalone Rust voice engine connected over private JSON-RPC IPC.

![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-blue) ![Electron](https://img.shields.io/badge/Electron-37-47848f) ![Rspack](https://img.shields.io/badge/Rspack-2-f4b942) ![Rust](https://img.shields.io/badge/Rust-stable-red)

## Features

- Push-to-talk dictation and assistant mode with global shortcuts
- Voice-to-action command execution
- Local settings, conversations, clipboard history, dictionary, notes, and snippets
- Configurable shortcuts, language preferences, microphone, and local Whisper model
- Native tray, deep links, autostart, single-instance handling, and auto-updates
- Sandboxed renderer with a narrow Electron preload API

## Prerequisites

Windows:

- Windows 10/11 (64-bit)
- Node.js 20+
- Rust stable
- Visual Studio Build Tools with the C++ workload

macOS:

- macOS 10.15+
- Node.js 20+
- Rust stable
- Xcode Command Line Tools
- Microphone and Accessibility permissions

Linux also needs the ALSA development package used by `cpal`.

## Quick Start

```bash
npm install
npm run desktop:dev
```

Dictation runs locally with Whisper and does not require an API key. During first-run model setup, ListenOS downloads the default `base.en` model into the per-user ListenOS models directory:

```text
<user data directory>/ListenOS/models
```

You can choose or download another supported local model later from `Settings -> System`.

Optional runtime configuration can still be placed in `.env.local`:

```env
LISTENOS_REQUIRE_CONFIRMATION=false
```

## Build

```bash
npm run desktop:build
```

Platform-specific bundles:

```bash
npm run desktop:build:windows
npm run desktop:build:mac
npm run desktop:build:linux
```

Installers and update metadata are written to `dist/electron/`.

## Default Shortcuts

| Action | Default | Behavior |
|---|---|---|
| Hold-to-talk | `Ctrl+Space` | Hold to record, release to process |
| Assistant mode | `Ctrl+Alt+Space` | Toggle hands-free listening |

Both shortcuts are configurable under `Settings -> General`.

## Architecture

```text
ListenOS/
|-- electron/
|   |-- main.cjs          # Windows, tray, lifecycle, updates, backend process
|   `-- preload.cjs       # Sandboxed renderer API
|-- src/
|   |-- app/              # Dashboard and assistant screen modules
|   |-- components/
|   `-- lib/desktop.ts    # Typed command and event bridge
|-- backend/
|   `-- src/
|       |-- ipc.rs        # Line-delimited JSON-RPC server
|       |-- commands/     # Voice and automation command handlers
|       |-- audio/
|       |-- transcription/ # Local Whisper model management and inference
|       `-- streaming/
`-- scripts/
    `-- electron-dev.mjs  # Rspack + Electron development launcher
```

Electron owns desktop lifecycle concerns. The Rust child process owns audio capture, local transcription, persistence, global hotkeys, and native system automation. Renderer code cannot access Node.js or spawn arbitrary processes directly.

## Scripts

| Command | Description |
|---|---|
| `npm run dev` | Start only the Rspack development server |
| `npm run desktop:dev` | Start the complete Electron app |
| `npm run build` | Bundle the React renderer with Rspack |
| `npm run backend:build` | Compile the Rust backend in release mode |
| `npm run desktop:build` | Build the current platform package |
| `npm run lint` | Run ESLint |

## Releases

Tagged releases build Electron installers on Windows, macOS, and Linux. Electron Builder produces the `latest*.yml` metadata consumed by `electron-updater`, and the release workflow publishes it to Cloudflare R2.

Required GitHub secrets:

- `CLOUDFLARE_R2_ACCESS_KEY_ID`
- `CLOUDFLARE_R2_SECRET_ACCESS_KEY`
- `CLOUDFLARE_R2_ENDPOINT`
- `CLOUDFLARE_R2_BUCKET`
- `CLOUDFLARE_R2_PUBLIC_BASE_URL`

Version helpers:

- `npm run release:prepare -- <version>`
- `npm run version:sync`

## License

Proprietary software. See [LICENSE](LICENSE). Third-party attributions are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
