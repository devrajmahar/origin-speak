# GPUI Native Runtime Architecture

Origin Speak uses GPUI only for the resident runtime surface. The product has no general-purpose desktop dashboard or settings UI.

The production split is:

```text
origin                           origin-runtime
CLI manager                     silent resident process
-----------                     -----------------------
setup/config                    one global dictation hotkey
models/microphone               microphone capture
hotkey/autostart                local Whisper inference
start/stop/restart              focused-app text delivery
update/uninstall                compact GPUI status overlay
```

The production architecture contains no Electron, React, Node.js, browser/WebView shell, HTTP bridge, or JSON-RPC bridge. Both binaries call the Rust core through typed in-process APIs.

## Resident GPUI surface

The runtime keeps a hidden GPUI/event-loop anchor as needed by the platform and global-hotkey integration. Its only visible UI is a compact, non-activating status overlay with four semantic states:

- `Listening`: audio capture has actually started.
- `Processing`: capture has ended and transcription/delivery is running.
- `Success`: dictated text was delivered.
- `Error`: capture, transcription, or delivery failed.

The overlay must remain mouse-transparent/non-activating so it never steals focus from the target application. There are no confirmation controls, hands-free controls, dashboard pages, settings dialogs, or onboarding windows.

## Runtime rules

- Emit `Listening` only after CPAL successfully opens and starts the microphone stream.
- Releasing the dictation hotkey snapshots capture quickly; Whisper inference and delivery continue away from the command/event loop.
- Audio-level animation runs only while listening feedback is visible.
- Model inference, model download, hashing, SQLite work, update I/O, and other blocking operations do not run on the GPUI render thread.
- Model readiness must use cached verified identity on hot paths; unchanged multi-GB files must not be rehashed for every shortcut press or status query.
- The runtime owns no assistant/action/conversation state.
- Persistent configuration is changed by the `origin` manager and consumed by the runtime on launch/reload.

## Core data flow

```text
global hotkey press
      |
      v
CPAL capture  ---> compact Listening overlay
      |
hotkey release
      |
      v
audio snapshot ---> Processing overlay
      |
      v
local Whisper + recognition dictionary hints
      |
      v
minimal trim/noise filtering
      |
      v
focused-app text delivery
      |
      +---- success ----> Success overlay
      `---- failure ----> Error overlay + clipboard/recovery buffer when available
```

## CLI/runtime ownership

`origin` owns setup, model selection/download/removal, microphone selection/test, the dictation hotkey, autostart preference/integration, runtime lifecycle, diagnostics, updates, and uninstall.

`origin-runtime` owns only resident dictation execution and its compact status feedback.

Autostart must target `origin-runtime` directly. The manager must not be kept resident merely to host UI or settings.

## Release-readiness checks

Before a public release, verify:

- The global dictation shortcut works while another application owns focus.
- `Listening` is never shown before microphone capture has successfully started.
- The overlay never steals focus and transitions cleanly through Listening/Processing/Success/Error.
- Repeated dictation does not leave the runtime stuck in listening or processing state.
- Model/config/microphone/hotkey operations work through `origin` without any UI dependency.
- Existing verified model files are not repeatedly SHA-256 hashed on hot runtime/status paths.
- Recognition dictionary hints affect Whisper spelling without introducing semantic command behavior.
- Uninstall removes exact app-owned model/data roots by default and preserves them only with explicit `--keep-data`.
- Windows release trust status is explicit: signed/timestamped when the configured signing inputs are complete, otherwise unsigned.
- macOS release trust status is explicit: Developer ID signed/notarized when the configured Apple inputs are complete, otherwise ad-hoc signed and non-notarized. Microphone + Accessibility behavior still requires real-device validation.

Native Linux packaging remains intentionally unsupported until the passive overlay behavior and native install/autostart lifecycle are safe across supported Wayland/X11 environments.
