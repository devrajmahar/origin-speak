# macOS Smoke Test Checklist

Use this checklist on a real macOS machine after installing from the generated DMG.

## Install and Launch

1. Open the `.dmg` and drag `ListenOS.app` into `Applications`.
2. Launch `ListenOS.app` from `Applications`.
3. Verify both windows initialize:
   - Dashboard window
   - Assistant overlay chip

4. Confirm onboarding appears on first launch and can be completed.
5. Relaunch app and confirm onboarding does not re-open after completion.

## Permissions

1. Grant **Microphone** permission.
2. Grant **Accessibility** permission (for keyboard/mouse automation).
3. Confirm the app works after permissions are granted without restart loops.

## Shortcut Flows

1. Verify hold-to-talk default: `Ctrl+Space`.
2. Hold `Ctrl+Space`, speak a short dictation sentence, release, and confirm text is typed.
3. Verify assistant mode default: `Ctrl+Alt+Space`.
4. Press `Ctrl+Alt+Space` once to start handsfree mode.
5. Speak and confirm dictation/command behavior.
6. Press `Ctrl+Alt+Space` again and confirm handsfree mode stops.
7. Change both shortcuts in `Settings -> General`, then re-test both paths.

## Dashboard and Tools

1. Open each dashboard page and verify it renders:
   - Conversation
   - Commands
   - Clipboard
   - Integrations
   - Dictionary
   - Snippets
   - Tone
2. Confirm notes page is not present in sidebar navigation.
3. Open `Settings -> System` and verify the local transcription model can be selected, downloaded, and reports `Ready` after installation.

## Files and System Actions

1. Run a safe file action (example: count Downloads items).
2. Run screenshot action and verify:
   - Screenshot file is created
   - Target folder opens as expected

## Overlay and UX

1. Confirm overlay remains centered and visible.
2. Confirm listening/processing states are visible in the chip.
3. Confirm voice waveform reacts while speaking.

## Theme and Visibility

1. Switch macOS appearance Light/Dark and relaunch app if needed.
2. Confirm dashboard follows system theme automatically.
3. Confirm text contrast is readable in both themes (no invisible text).
4. Confirm borders/dividers are visible and consistent across dashboard surfaces.

## Stability

1. Keep app running for at least 10 minutes with repeated commands.
2. Verify no crashes and no stuck listening state.
3. Quit and relaunch; confirm settings persist.
