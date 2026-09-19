# CLI management architecture

Origin Speak is split into a console `origin` manager and a silent resident `origin-runtime` process. This is the product architecture, not a migration shim: there is no dashboard/settings/onboarding application to keep alive.

## Responsibilities

`origin` owns management operations:

```text
origin setup
origin status
origin doctor
origin config ...
origin dictionary ...
origin model ...
origin mic ...
origin hotkey ...
origin autostart ...
origin start
origin stop
origin restart
origin update ...
origin uninstall ...
origin version
```

`origin-runtime` owns only the resident dictation path:

```text
one global dictation hotkey
microphone capture
local Whisper transcription
recognition dictionary hints
focused-app text delivery
compact Listening / Processing / Success / Error overlay
```

The runtime must not grow assistant intent/action routing, conversations, confirmation workflows, clipboard history, notes, snippets, custom commands, or a settings/dashboard UI.

## Output contract

`--json` is global and emits one plain JSON object with no ANSI control sequences, spinner frames, or interactive prompts. Human terminal downloads use an in-place progress line with transferred bytes, percentage, throughput, and ETA. Destructive commands must not hang waiting for stdin in a non-interactive pipe/CI session.

## Setup and configuration

`CoreCliExecutor` talks directly to `origin_speak_lib`. Setup is idempotent: existing valid models and matching selections are preserved; missing models are downloaded and verified; requested microphone/autostart values are applied only when needed.

When `origin setup` is run directly in an interactive terminal with no explicit setup choices, it provides a guided terminal flow. It shows detected CPU/RAM/accelerator status, the hardware-based model recommendation and model catalog, microphone choices, the dictation hotkey, and autostart preference. It shows a summary before persisting changes. Missing model downloads then report live progress instead of leaving setup apparently idle. `--json`, redirected input/output, and setup invocations with explicit choices remain deterministic and non-interactive.

The manager installs to the per-user manager path. On Windows setup adds only that exact directory to the per-user `PATH`, preserving unrelated entries and broadcasting the environment change; uninstall removes only the matching entry. Unix uses `~/.local/bin/origin` and reports whether that conventional directory is already on `PATH` without editing shell startup files.

The persisted backend configuration is intentionally small and voice-to-text specific: dictation hotkey, selected microphone, GPU preference, autostart preference, and transcription source language. Old richer JSON configs remain readable because unknown legacy fields are ignored.

`origin dictionary` manages only recognition hints used by Whisper prompting: list, add, update, and remove words with optional pronunciation text. The current Canary-Qwen native backend does not expose a prompt/hotword extension, so these hints are not applied when Canary-Qwen is selected. It does not restore snippets, custom commands, or voice actions.

Model operations use a backend-aware catalog, immutable upstream revisions, SHA-256 verification, resumable `.part` downloads, current/legacy model-root discovery, and safe per-model removal. Whisper models run through whisper.cpp; `canary-qwen-2.5b` runs in-process through transcribe.cpp using a pinned Q8_0 GGUF. Canary-Qwen is English-only and long dictation is segmented below its upstream 40-second training window before deterministic overlap de-duplication. A retry resumes from the persisted partial length without a full pre-resume hash pass; a completed partial is only promoted after exact SHA-256 verification. The final multi-gigabyte hash runs off the async executor so terminal `Verifying` feedback stays responsive.

## Hardware recommendation

`native/src/system_capabilities.rs` reads CPU architecture/count and physical RAM without shell commands. Windows RAM comes from `GlobalMemoryStatusEx`; macOS uses `sysctlbyname`; Linux reads `/proc/meminfo` for development only. Accelerator state is not guessed from CPU architecture or environment variables.

The deterministic recommendation is latency-oriented: constrained systems choose `tiny.en`, balanced systems choose `base.en`, and machines with more verified CPU/RAM headroom may choose `small.en`. Large models should not be selected automatically without a trustworthy native accelerator capability signal.

## Runtime lifecycle

Windows lifecycle control uses native named-object/process primitives and starts `origin-runtime.exe` with no console window. macOS/Linux lifecycle control uses the resident Unix socket at the backend local-data root (`runtime.sock`) with `PING`/`PONG` and `QUIT`/`OK` messages. No platform uses `cmd.exe`, PowerShell, `pkill`, `osascript`, shell scripts, or process-name guessing for product lifecycle control.

Autostart targets the resident runtime directly. The console manager is invoked only when the user or automation needs a management operation.

Model management is explicit but compact: `origin model list` shows catalog state, `origin model installed` filters to local models, `origin model install <id>` downloads without changing the default, `origin model use <id>` installs when necessary and then makes that model the default, and `origin model remove <id>` deletes a non-default local model. The older `download` and `select` spellings remain compatibility aliases.

Mutating CLI operations persist through the backend typed APIs. If an installed resident runtime is already running, model switching, microphone selection, dictation-hotkey changes, and config set/reset automatically restart it so the new persisted state takes effect immediately.

## Updates and release artifacts

`bootstrap-update.json` describes separate manager/runtime release payloads. Canonical downloads come from `https://github.com/devrajmahar/origin-speak/releases/download/v<version>/...` and require credential-free HTTPS, size bounds, platform/architecture validation, and SHA-256 verification before staging/replacement.

Windows release output is a standalone `origin` executable plus a standalone `origin-runtime` executable, published as `origin-speak-*` and `origin-speak-runtime-*` payloads with associated hashes and trust metadata. When the complete Windows signing input set is available they are signed/timestamped; otherwise the GitHub Release marks them unsigned.

macOS release output likewise keeps manager and resident runtime artifacts separate. A complete Apple release credential set produces Developer ID signed/notarized artifacts; otherwise the workflow publishes ad-hoc signed, non-notarized artifacts and records that status in the GitHub Release.

Self-replacement must happen after the running manager exits. The Windows implementation may use a narrowly scoped post-exit helper mode; it must not use shell deletion commands.

`origin update` is the normal apply-now command: it checks the canonical GitHub Release manifest, downloads and SHA-256 verifies both manager and runtime payloads, then performs the platform-safe replacement while preserving prior runtime running/stopped state. `origin upgrade` is an alias. `origin update check` is the non-mutating availability check. The older `origin update stage` spelling remains accepted only as a compatibility alias for the apply flow. On Windows the running manager cannot replace itself, so a successful invocation reports `update_scheduled` once the post-exit helper is armed and a durable pending result is recorded. The helper appends its completed/failed result after the manager exits; the next `origin status` or update invocation surfaces that result and consumes the record instead of silently losing detached-helper failures.

## Uninstall

Uninstall removes app-owned data by default, including downloaded transcription models, config, databases/runtime state, installed runtime/manager targets, recognized legacy ListenOS model roots/update payloads, and stale Origin Speak update payloads. `--keep-data` is the explicit opt-out.

### Migrating a pre-Origin-Speak macOS install

Historical `ListenOS.app` releases registered the main application itself with macOS `SMAppService` and did not expose a bounded command-line unregister operation. Origin Speak therefore does not launch that old executable with new maintenance flags or delete its bundle behind a potentially registered login item. If `~/Applications/ListenOS.app` is still present, disable **ListenOS** in **System Settings → General → Login Items**, quit ListenOS, remove the old `ListenOS.app` bundle, and rerun `origin setup`. Existing ListenOS models and user data remain eligible for the normal safe migration after the old bundle is retired.

Deletion code validates exact product-owned paths, avoids deleting parent directories, and does not traverse symlinks/reparse points. Non-interactive destructive uninstall requires `--yes`.

The runtime is stopped and autostart registration is removed before installed files/data are deleted.

## Platform caveats

- Windows: resident launch must remain console-free; Vulkan GPU builds require the Vulkan SDK at build time.
- macOS: real-device validation must cover Microphone and Accessibility permission prompts plus both trust modes. The published runtime is an `Origin Speak.app` ZIP that is either Developer ID signed/notarized or ad-hoc signed as identified by release trust metadata. The CLI verifies the downloaded payload hash, extracts it with `/usr/bin/ditto`, validates the exact `com.originspeak.app` bundle identity and `Contents/MacOS/origin-runtime` executable layout, verifies the code signature, and transactionally installs it at `~/Applications/Origin Speak.app`; updates retain the prior bundle until the replacement runtime passes its READY gate.
- Linux: development code exists, but production packaging/autostart/overlay behavior remains unsupported until the Wayland/X11 passive-overlay and install lifecycle constraints are resolved.
