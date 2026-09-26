# Offline transcription evaluation

Set `ORIGIN_SPEAK_DEBUG_AUDIO_DIR` before starting the resident to opt in to debug capture. Each dictation writes two PCM16 mono WAV files: the application capture at the microphone's native rate and the final conditioned 16 kHz model input. No files are written when the variable is unset. The file writes run in the existing blocking transcription worker, not on the GPUI render thread.

Place a reference transcript beside each WAV with the same stem and a `.txt` extension, then run:

```text
cargo run --manifest-path backend/Cargo.toml --example evaluate -- large-v3 C:\path\to\clips --language en
cargo run --manifest-path backend/Cargo.toml --example evaluate -- qwen3-asr-1.7b C:\path\to\clips --cpu --language auto
```

The chosen model must already be installed. The evaluator accepts PCM16 WAV input, sends every clip through the same `TranscriptionService` resampling, conditioning, model-loading, inference, and delivery-formatting path as the app, and prints per-file audio duration, latency, real-time factor, transcript, and normalized word error rate. WER lowercases and removes punctuation before word-level Levenshtein distance. Files without references are transcribed and timed but excluded from aggregate WER.

For controlled comparisons, keep the same WAV/reference corpus and run one model or decode configuration at a time. Record both aggregate WER and RTF; do not compare WER across different clip sets.
