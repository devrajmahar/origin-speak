# Origin Speak

Local, open-source voice-to-text for Windows and macOS. Whisper and Canary-Qwen 2.5B run on your machine; no API key or cloud transcription service is required.

## Install

### Windows

Open PowerShell:

```powershell
$p = "$env:TEMP\origin.exe"
irm https://github.com/devrajmahar/origin-speak/releases/latest/download/origin-windows-x86_64.exe -OutFile $p
& $p setup
```

Open a new terminal after setup:

```powershell
origin status
```

### macOS

Open Terminal:

```bash
d="$(mktemp -d)"
p="$d/origin"
curl -fL https://github.com/devrajmahar/origin-speak/releases/latest/download/origin-macos-universal -o "$p"
chmod +x "$p"
"$p" setup
rm -rf "$d"
```

Then run `origin status`. Allow Microphone and Accessibility permissions when macOS asks.

## Use

Default shortcut: **Shift + Space**.

Hold the shortcut, speak, then release it. Origin Speak transcribes locally and types into the focused application.

## Common commands

| What you want to change | Command |
|---|---|
| Show status | `origin status` |
| Run diagnostics | `origin doctor` |
| List models | `origin model list` |
| Show installed models only | `origin model installed` |
| Check selected model / GPU or CPU | `origin model status` |
| Install a model | `origin model install <id>` |
| Switch/default to a model | `origin model use <id>` |
| Remove a model | `origin model remove <id>` |
| List microphones | `origin mic list` |
| Show microphone | `origin mic status` |
| Select microphone | `origin mic select "<name>"` |
| Use system-default microphone | `origin mic select default` |
| Show shortcut | `origin hotkey show` |
| Change shortcut | `origin hotkey set Ctrl+Shift+Space` |
| Enable GPU preference | `origin config set use_gpu true` |
| Force CPU | `origin config set use_gpu false` |
| Enable autostart | `origin autostart enable` |
| Disable autostart | `origin autostart disable` |
| Start runtime | `origin start` |
| Stop runtime | `origin stop` |
| Restart runtime | `origin restart` |
| Install latest update | `origin update` or `origin upgrade` |
| Check for update only | `origin update check` |
| Uninstall | `origin uninstall` |

A running resident automatically restarts when a changed model, microphone, hotkey, or relevant config value needs to take effect.

## Models

Start with the catalog. Row numbers are visual aids only; commands always use the stable model ID from the `MODEL` column.

```text
Origin Speak · Transcription Models

 #  MODEL               NAME                         SIZE        STATUS
 ──────────────────────────────────────────────────────────────────────────────
 1  tiny.en             Tiny English                 —           Available
 2  base.en             Base English                 —           Recommended
 3  small.en            Small English                —           Available
…

Models: 11 available · 0 installed · 1 default

Actions
  Install or switch model    origin model use <model-id>
  Install without switching  origin model install <model-id>
  View installed models      origin model installed
  View active model / GPU    origin model status
  Remove installed model     origin model remove <model-id>
```

Install and switch in one command:

```text
origin model use canary-qwen-2.5b
origin model status
```

`origin model use <id>` validates the ID, installs and verifies the model when necessary, then makes it the default. An already-installed model is not downloaded again, and selecting the current default is a no-op. A running resident restarts only after the selection has been saved. If restart fails, the CLI explains that the model change succeeded and how to retry the runtime.

`origin model install <id>` downloads a model without changing the default. `origin model installed` shows only models occupying storage, their measured file sizes, the default marker, and a total when every size is available.

Remove a model that is no longer the default:

```text
origin model use large-v3
origin model remove base.en
```

Origin Speak never silently changes the default during removal. Attempting to remove the default model is rejected with the exact switch and retry commands. An unknown ID reports close catalog matches; a failed download leaves the prior default unchanged.

Run `origin model --help` for the complete workflow. The older `download`, `select`/`switch`, and `delete`/`uninstall` spellings remain accepted as compatibility aliases.

Canary-Qwen 2.5B is English-only. Whisper models include English-only and multilingual variants.

## Check whether transcription is actually using GPU or CPU

Run:

```text
origin model status
```

GPU example:

```text
Origin Speak · Model Status

Default model    canary-qwen-2.5b
Installed        Yes
GPU preference   enabled
Active compute   GPU
Accelerator      NVIDIA GeForce RTX 4080
```

CPU fallback example:

```text
Active compute   CPU
Fallback reason  <reason>
```

`GPU preference: enabled` only means Origin Speak should try the GPU. `Active compute` reports what the resident actually loaded.

Right after startup, the model may not be loaded yet:

```text
Active compute   pending (model not loaded yet)
```

Run `origin model status` again after a few seconds or after the first dictation.

## Change the shortcut

Default:

```text
Shift+Space
```

Change it at any time:

```text
origin hotkey set Ctrl+Shift+Space
```

If the resident is running, Origin Speak restarts it automatically so the new binding becomes active.

## Updates

```text
origin update
```

`origin upgrade` does the same thing. Downloads show transferred bytes, percentage, throughput, ETA, and a separate verification phase.

## Uninstall

Remove Origin Speak, installed models, and app-owned data:

```text
origin uninstall
```

Keep models/config intentionally:

```text
origin uninstall --keep-data
```

## Privacy and acceleration

Origin Speak performs transcription locally. Supported execution paths are CPU, Vulkan on Windows, and Metal on macOS. Model downloads are pinned and verified before installation.

## Development

Implementation and contributor documentation:

- [SETUP.md](SETUP.md)
- [CLI management](docs/cli-management.md)
- [Native architecture](docs/gpui-native-architecture.md)
- [Contributing](CONTRIBUTING.md)

## License

Origin Speak is released under the [MIT License](LICENSE).
