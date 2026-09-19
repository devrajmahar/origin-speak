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
| Check selected model / GPU or CPU | `origin model status` |
| Download a model | `origin model download <id>` |
| Select a model | `origin model select <id>` |
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

Example:

```text
origin model download canary-qwen-2.5b
origin model select canary-qwen-2.5b
origin model status
```

Canary-Qwen 2.5B is English-only. Whisper models include English-only and multilingual variants.

## Check whether transcription is actually using GPU or CPU

Run:

```text
origin model status
```

GPU example:

```text
selected: canary-qwen-2.5b
gpu_preference: enabled
compute: GPU
compute_status: ready
accelerator: NVIDIA GeForce RTX 4080
```

CPU fallback example:

```text
compute: CPU
compute_fallback: <reason>
```

`gpu_preference: enabled` only means Origin Speak should try the GPU. The `compute` field reports what the resident actually loaded.

Right after startup, the model may not be loaded yet:

```text
compute: pending (model not loaded yet)
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

Origin Speak is released under the [MIT License](LICENSE). Third-party notices are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
