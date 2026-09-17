# GPUI Native Architecture

ListenOS is a single-process Rust desktop application built with GPUI. ListenOS owns its visual component system; `gpui-base` provides unstyled interaction infrastructure beneath application-owned components.

## Runtime architecture

```text
global shortcuts / native windows / tray / updater
                         |
                         v
                     GPUI app
                         |
                         v
              ListenOS Rust application core
              |       |        |       |
            audio   whisper   state   delivery
```

The GPUI application links the voice engine as the `voice_os_lib` Rust library and invokes typed operations directly.

## UI foundation

```text
ListenOS UI / components
          |
          v
      gpui-base
          |
          v
         GPUI
```

- ListenOS owns colors, spacing, radii, typography, motion, component composition, and visual states.
- `gpui-base` supplies unstyled focus, input, selection, scrolling, dialogs, switches, and related interaction primitives where useful.
- Listening, processing, success, and error feedback use one compact GPUI status box. It is non-activating and mouse-transparent so dictation never steals focus from the application underneath it.
- Confirmation and hands-free controls use a separate compact non-activating surface only while pointer input is actually required.
- Reusable text styles must define layout bounds, wrapping/truncation, and readable scaling behavior.

## Runtime rules

- Audio capture, model inference, model downloads, persistence, and update I/O never block the GPUI render thread.
- Global shortcut press transitions visible state before expensive capture or inference work begins.
- Releasing hold-to-talk transitions through processing to success, error, or confirmation state based on the core result.
- Audio-level animation schedules frames only while an animated overlay state is visible.
- Settings mutations use typed core APIs and persist through the Rust configuration store.
- Native shell responsibilities include tray/menu-bar lifecycle, autostart, single-instance activation, `listenos://` protocol activation, updater behavior, and platform permission integration.

## Release-readiness checks

Before a public native release, verify these behaviors on the target operating systems:

- Windows global shortcuts work while another application owns focus.
- The compact status overlay never steals focus; only the compact control surface accepts pointer input when confirmation or hands-free controls are visible.
- Multiple monitors and mixed-DPI arrangements do not clip or offset overlay surfaces.
- First-run model download and microphone setup complete in the native UI.
- Settings survive process restart, including shortcut registration and shell settings where supported.
- Local speech, command routing, delivery, dictionary, snippets, notes storage, and conversation tests remain green.
- Windows installer signing and timestamp validation succeed before publication.
- macOS native windowing, microphone permission, Accessibility automation, global shortcuts, tray/menu-bar behavior, deep-link activation, Developer ID signing, notarization, and package installation are validated on a macOS host.

Native Linux packaging remains intentionally unsupported until a safe cross-backend overlay/input solution exists for both Wayland and X11.
