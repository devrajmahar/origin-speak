# Contributing to ListenOS

ListenOS is proprietary software. Coordinate with maintainers before starting major work.

## Workflow

1. Create a focused branch from the latest main branch.
2. Keep changes scoped to one problem/feature per PR.
3. Run local checks before opening a PR:
   - `cargo fmt --manifest-path backend/Cargo.toml -- --check`
   - `cargo test --manifest-path backend/Cargo.toml --locked`
   - `cargo fmt --manifest-path native/Cargo.toml -- --check`
   - `cargo check --manifest-path native/Cargo.toml --locked`
   - `cargo run --manifest-path native/Cargo.toml` (quick manual sanity check for UI/runtime changes)
4. Open a PR with:
   - Problem statement
   - Implementation summary
   - Validation steps and results
   - Screenshots/video for UI changes

## Product Constraints

- Desktop app is self-hosted first: do not reintroduce login-gated dashboard flows.
- Keep first-run onboarding behavior intact unless a task explicitly requests temporary disablement.
- Dictation is local-first and uses local Whisper models; do not add an API-key requirement to the transcription flow.
- Voice flow should execute and transcribe without spoken voice playback.
- Avoid Bluetooth hands-free microphone routing that can hijack headphone output.

## Code Guidelines

- Match existing Rust style and naming.
- Prefer minimal, root-cause fixes over broad refactors.
- Keep UX responsive; avoid adding extra latency in hotkey/audio paths.
- Update `README.md` when behavior, setup, or configuration changes.

## Security and Privacy

- Never commit secrets or real API keys.
- Treat local data handling changes as high impact and document them in PR notes.
