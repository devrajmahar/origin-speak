# ListenOS Setup

ListenOS packages Electron, a React renderer built with Rspack, and a Rust native backend into one desktop app. No separate server or cloud login is required.

## Install

Install Node.js 20+, Rust stable, and the platform compiler toolchain, then run:

```bash
npm install
```

Windows requires Visual Studio Build Tools with C++. macOS requires Xcode Command Line Tools. Linux requires ALSA development headers.

## Configure

Dictation uses local Whisper inference and does not require a transcription API key. During first-run model setup, ListenOS downloads the default `base.en` model into the per-user models directory:

```text
<user data directory>/ListenOS/models
```

The exact user data root follows the operating system's standard application-data location. Model selection, download state, and runtime status are available in `Settings -> System`.

If you want to change optional runtime behavior, copy `.env.example` to `.env.local`:

```env
LISTENOS_REQUIRE_CONFIRMATION=false
```

After a model is downloaded, transcription runs locally on the machine. Network access is only needed when downloading a model.

## Develop

```bash
npm run desktop:dev
```

This starts Rspack, Electron, and the Rust backend. Rust changes are rebuilt when the desktop app restarts.

## Package

```bash
npm run desktop:build:windows
npm run desktop:build:mac
npm run desktop:build:linux
```

Outputs are placed in `dist/electron/`.

## Default Shortcuts

- Hold-to-talk: `Ctrl+Space`
- Assistant mode: `Ctrl+Alt+Space`

Both are configurable in `Settings -> General`.
