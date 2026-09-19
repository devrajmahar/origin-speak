# macOS Smoke Test Checklist

Run this checklist on a real macOS 13+ machine using the release `origin` manager and `origin-runtime` artifacts. The product no longer relies on a DMG/dashboard installation flow.

## Install and setup

1. Start from the published `origin-speak-<version>-macos-universal` manager and check the GitHub Release trust note first. Expect either Developer ID signed/notarized or ad-hoc signed/non-notarized status. Do not manually extract or relocate the runtime app bundle.
2. Run `origin setup`. Verify it installs the resident runtime bundle at `~/Applications/Origin Speak.app` and the executable at `~/Applications/Origin Speak.app/Contents/MacOS/origin-runtime`, without opening dashboard/settings/onboarding UI.
3. Migration guard: place a valid historical `~/Applications/ListenOS.app` bundle in the legacy location and run `origin setup`. Confirm setup exits promptly before changing current Origin Speak install/autostart state, tells the user to disable ListenOS under **System Settings > General > Login Items**, and does not launch or delete the legacy bundle. After disabling the old login item, quitting ListenOS, and removing `ListenOS.app`, rerun setup and confirm legacy models/data remain migratable.
4. Run `/usr/bin/codesign --verify --deep --strict ~/Applications/Origin\ Speak.app` and verify the installed bundle remains valid after bootstrap extraction/swap.
5. Verify `/usr/bin/plutil -extract CFBundleIdentifier raw -o - ~/Applications/Origin\ Speak.app/Contents/Info.plist` prints `com.originspeak.app`.
6. Verify `origin status` identifies the resident runtime/configuration state.
7. Quit/re-run the manager and confirm setup remains idempotent rather than redownloading an already-valid model.

## Permissions

1. Grant **Microphone** permission to the resident runtime when macOS requests it.
2. Grant **Accessibility** permission required for typing/paste delivery into other applications.
3. Confirm denied permissions produce a clean error state rather than a launch/restart loop.

## Dictation shortcut

1. Verify the default macOS dictation shortcut is `Shift+Space`.
2. Hold `Shift+Space`, speak a short sentence, release, and confirm the sentence is delivered to the focused text field.
3. Verify there is no second assistant/hands-free shortcut.
4. Run `origin hotkey set <chord>`, verify the CLI reports that the running resident was restarted, and verify the new binding works.
5. Verify pressing the shortcut while microphone access is unavailable does not falsely show `Listening`.

## Overlay and focus behavior

1. On successful capture start, verify the compact overlay shows `Listening`.
2. On release, verify it transitions to `Processing`.
3. Verify successful text delivery produces the transient `Success` state.
4. Force a safe capture/transcription/delivery failure and verify the transient `Error` state.
5. Confirm the overlay never activates itself, changes the focused target, or blocks pointer interaction with the application underneath it.
6. Verify no dashboard/settings/onboarding/confirmation surfaces appear.

## Models and configuration

1. Run `origin model list` and verify supported local Whisper models are reported.
2. Download a small model, select it, restart the runtime, and confirm dictation works.
3. Re-run model status/list and confirm an unchanged verified model does not cause a visibly long full-file verification pause.
4. Interrupt a model download, retry it, and verify the safe partial download resumes when the server honors Range.
5. Run `origin mic list`, `origin mic status`, and `origin mic select <name>`.
6. Run `origin config list` and verify only current voice-to-text configuration is exposed.

## Resident lifecycle

1. Run `origin start`, `origin status`, `origin restart`, and `origin stop` in sequence.
2. Start the runtime again and verify only one resident instance owns the dictation hotkey.
3. Enable/disable autostart with `origin autostart ...` and confirm the maintenance request executes the bundled `Contents/MacOS/origin-runtime` binary so `SMAppService::mainAppService()` operates with the installed app-bundle identity.
4. Sign out/in (or otherwise exercise the real login-item path) and confirm the runtime launches from `~/Applications/Origin Speak.app`, not from a temporary/download location or the CLI manager.
5. Validate the platform-native lifecycle implementation; do not use `pkill`, `osascript`, or process-name guessing as a substitute.

## Update bundle swap

1. Install an older release, start the runtime, then run `origin update` (and separately verify the `origin upgrade` alias) for a newer release.
2. Confirm the resident runtime is stopped before the live app bundle is replaced.
3. Confirm the downloaded `origin-speak-runtime-<version>-macos-universal.zip` is SHA-256 verified before extraction.
4. Confirm extraction uses the release app bundle as one top-level `Origin Speak.app`, and the installed bundle still passes `codesign --verify --deep --strict`.
5. Confirm update leaves no `.Origin-Speak-extracting-*`, `.Origin-Speak-installing-*.app`, `.Origin-Speak-backup-*.app`, or `.Origin-Speak-transaction-*.marker` sibling paths after success.
6. Force a safe bundle-verification/install failure and confirm the previous `~/Applications/Origin Speak.app` is restored and remains launchable.
7. Simulate an interrupted owned transaction with the live `Origin Speak.app` missing and exactly one signed/verified `.Origin-Speak-backup-*.app` claimed by a valid Origin Speak transaction marker. Run `origin start`, `origin setup`, or uninstall and confirm the backup is restored before the command continues and owned stale transaction artifacts are removed.
8. With a valid live `Origin Speak.app`, leave owned stale backup/install/extraction artifacts plus their valid transaction marker and confirm the next setup/start removes them only after verifying the live bundle.
9. Place similarly named hidden directories beside the app without a valid Origin Speak ownership marker/schema and confirm setup/start leaves every colliding directory untouched.

## Delivery behavior

1. Test dictation into a native text field, browser text field, and code editor.
2. Confirm paste-based delivery restores prior clipboard text after a successful insertion when supported.
3. Force delivery failure and verify the runtime reports `Error` and retains/copies the transcript for recovery when possible.
4. Confirm spoken command-looking text is inserted literally rather than executed as an action.

## Uninstall

1. Use `origin uninstall --dry-run` if available in the current CLI flow and inspect the planned app-owned paths.
2. Run the normal uninstall and verify the resident runtime is stopped and SMAppService autostart is removed before the app bundle is deleted.
3. Force SMAppService unregister/maintenance to fail while `~/Applications/Origin Speak.app` still exists. Confirm uninstall fails, reports the unregister error, and leaves the app bundle in place for a retry.
4. Test a partial install where `Origin Speak.app` is missing and no verified owned backup is recoverable. Confirm uninstall reports actionable residual login-item state and never reports `uninstall_complete` until a signed bundle is restored and SMAppService unregister succeeds.
5. Confirm only the exact owned `~/Applications/Origin Speak.app` bundle is removed; an unrelated neighboring app or symlink must never be traversed/deleted.
6. Confirm app-owned configuration/database state and current/recognized legacy model roots are removed by default.
7. Repeat with `--keep-data --yes` and verify user data/models are retained only in that explicit mode.

## Stability

1. Keep `origin-runtime` resident for at least 10 minutes with repeated dictation cycles.
2. Verify no crash, stuck listening/processing state, duplicate insertion, or focus theft.
3. Restart the runtime and confirm selected model, microphone, hotkey, language preference, and autostart preference remain consistent.
