# Contributing to Origin Speak

Origin Speak is an open-source native Rust voice-to-text application released under the MIT License. The product consists of the `origin` CLI manager and the silent resident `origin-runtime`; the backend is a reusable local dictation engine. Contributions must not reintroduce an Electron, React, Node.js, browser/WebView, HTTP, or JSON-RPC compatibility layer.

Coordinate with maintainers before starting major architectural work.

## Workflow

1. Create a focused branch from the latest main branch.
2. Keep changes scoped to one problem/feature per PR.
3. Run local checks before opening a PR:
   - `cargo fmt --manifest-path backend/Cargo.toml -- --check`
   - `cargo test --manifest-path backend/Cargo.toml --locked`
   - `cargo fmt --manifest-path native/Cargo.toml -- --check`
   - `cargo check --manifest-path native/Cargo.toml --locked`
4. Open a PR with a problem statement, implementation summary, validation results, and screenshots/video only when the compact overlay itself changes.

## Product constraints

- The product is voice-to-text only. Do not add assistant intent routing, actions, command execution, conversations, confirmations, snippets, notes, clipboard history, or style/vibe transformation back into the production path.
- Keep transcription local with Whisper; do not add an API-key or cloud-transcription requirement.
- Keep one dictation shortcut. Do not add a second assistant/hands-free shortcut.
- Keep the resident UI limited to the compact `Listening`, `Processing`, `Success`, and `Error` overlay states.
- Models, microphones, hotkey, autostart, lifecycle, updates, and persisted configuration are managed through `origin`, not a dashboard/settings UI.
- Recognition dictionary hints remain a supported local STT feature.
- Avoid Bluetooth hands-free microphone routing when it would switch the headset into low-quality telephony mode or hijack output audio.
- Uninstall must remove app-owned data/models by default and must never recursively delete unvalidated parent directories or follow symlinks/reparse points.

## Code guidelines

- Match existing Rust style and naming.
- Prefer minimal root-cause fixes over broad abstraction layers.
- Keep startup and hotkey/audio paths low-latency; never hash multi-GB model files repeatedly on hot readiness paths.
- Keep model downloads immutable-revision pinned, SHA-256 verified, resumable, and atomically installed.
- Keep blocking audio/model/download work away from the GPUI render thread.
- Update `README.md`, `SETUP.md`, and relevant architecture docs when CLI/runtime behavior changes.

## Security and privacy

- Never commit secrets or real API keys.
- Treat model integrity, update integrity, text injection, local databases, and uninstall path validation as security-sensitive code.
- Document any change to app-owned data roots or deletion behavior in the PR.
