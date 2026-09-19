use std::ops::Range;
use std::path::Path;

use transcribe_cpp::{
    Backend, DeviceType, Error as TranscribeError, Model, ModelOptions, RunOptions, Session,
    SessionOptions, TimestampKind,
};

use super::{BackendSelection, TranscriptionComputeBackend, WHISPER_SAMPLE_RATE};

const MAX_CHUNK_SECONDS: usize = 38;
const CHUNK_OVERLAP_SECONDS: usize = 2;
const MAX_STITCH_OVERLAP_WORDS: usize = 24;

pub(super) struct Runtime {
    session: Session,
}

impl Runtime {
    pub(super) fn load(
        path: &Path,
        gpu_requested: bool,
    ) -> Result<(Self, BackendSelection), String> {
        let requested_backend = if gpu_requested {
            Backend::Auto
        } else {
            Backend::Cpu
        };
        let model = Model::load_with(
            path,
            &ModelOptions {
                backend: requested_backend,
                device: None,
            },
        )
        .map_err(|error| format!("Failed to load Canary-Qwen model: {error}"))?;

        let architecture = model.arch();
        if architecture != "canary_qwen" {
            return Err(format!(
                "Canary-Qwen artifact has unexpected architecture '{architecture}'"
            ));
        }

        let backend = loaded_backend(&model, gpu_requested);
        let session = model
            .session_with(&SessionOptions {
                n_threads: super::cpu_thread_count(),
                ..SessionOptions::default()
            })
            .map_err(|error| format!("Failed to create Canary-Qwen inference session: {error}"))?;
        Ok((Self { session }, backend))
    }

    pub(super) fn transcribe(
        &mut self,
        audio: &[f32],
        dictionary_hints: &[(String, Option<String>)],
    ) -> Result<String, TranscribeError> {
        if !dictionary_hints.is_empty() {
            log::debug!(
                "Canary-Qwen does not expose an initial-prompt path; {} recognition dictionary hints are not passed to this model",
                dictionary_hints.len()
            );
        }

        let ranges = chunk_ranges(audio.len(), WHISPER_SAMPLE_RATE as usize);
        let mut transcripts = Vec::with_capacity(ranges.len());
        for range in ranges {
            let options = RunOptions {
                timestamps: TimestampKind::None,
                language: None,
                ..RunOptions::default()
            };
            // Canary-Qwen is English-only and does not expose language
            // conditioning. Keep the language unset rather than asking the
            // generic runtime to apply a capability the family does not have.
            let transcript = self.session.run(&audio[range], &options)?;
            transcripts.push(transcript.text);
        }
        Ok(stitch_transcripts(&transcripts))
    }
}

pub(super) fn should_retry_on_cpu(error: &TranscribeError) -> bool {
    matches!(
        error,
        TranscribeError::Backend(_) | TranscribeError::OutOfMemory(_)
    )
}

pub(super) fn planned_backend(gpu_requested: bool) -> BackendSelection {
    if !gpu_requested {
        return BackendSelection {
            active: TranscriptionComputeBackend::Cpu,
            accelerator: None,
            fallback_reason: None,
        };
    }

    let gpu = transcribe_cpp::devices()
        .into_iter()
        .find(|device| device_metadata_is_gpu(device.device_type, &device.kind));
    match gpu {
        Some(device) => BackendSelection {
            active: TranscriptionComputeBackend::Gpu,
            accelerator: Some(device_label(&device.kind, &device.description, &device.name)),
            fallback_reason: None,
        },
        None => BackendSelection {
            active: TranscriptionComputeBackend::Cpu,
            accelerator: None,
            fallback_reason: Some(
                "GPU is enabled, but the Canary-Qwen runtime found no usable GPU device. Using CPU."
                    .to_string(),
            ),
        },
    }
}

fn loaded_backend(model: &Model, gpu_requested: bool) -> BackendSelection {
    let backend_name = model.backend();
    let device = model.device().ok();
    let is_gpu = device
        .as_ref()
        .is_some_and(|device| device_metadata_is_gpu(device.device_type, &device.kind));

    if is_gpu {
        let accelerator = device
            .map(|device| device_label(&backend_name, &device.description, &device.name))
            .or_else(|| Some(backend_name.clone()));
        BackendSelection {
            active: TranscriptionComputeBackend::Gpu,
            accelerator,
            fallback_reason: None,
        }
    } else {
        BackendSelection {
            active: TranscriptionComputeBackend::Cpu,
            accelerator: None,
            fallback_reason: gpu_requested.then(|| {
                format!(
                    "GPU was requested for Canary-Qwen, but the runtime selected '{backend_name}'. Using CPU."
                )
            }),
        }
    }
}

fn device_metadata_is_gpu(device_type: DeviceType, kind: &str) -> bool {
    matches!(device_type, DeviceType::Gpu | DeviceType::Igpu)
        || matches!(
            kind.to_ascii_lowercase().as_str(),
            "metal" | "vulkan" | "cuda" | "rocm" | "sycl" | "gpu"
        )
}

fn device_label(backend: &str, description: &str, name: &str) -> String {
    let detail = if description.trim().is_empty() {
        name.trim()
    } else {
        description.trim()
    };
    if detail.is_empty() || detail.eq_ignore_ascii_case(backend) {
        backend.to_string()
    } else {
        format!("{backend} · {detail}")
    }
}

fn chunk_ranges(sample_count: usize, sample_rate: usize) -> Vec<Range<usize>> {
    if sample_count == 0 || sample_rate == 0 {
        return Vec::new();
    }
    let max_samples = MAX_CHUNK_SECONDS * sample_rate;
    if sample_count <= max_samples {
        return vec![0..sample_count];
    }

    let overlap = CHUNK_OVERLAP_SECONDS * sample_rate;
    let step = max_samples.saturating_sub(overlap).max(1);
    let mut ranges = Vec::new();
    let mut start = 0usize;
    loop {
        let end = start.saturating_add(max_samples).min(sample_count);
        ranges.push(start..end);
        if end == sample_count {
            break;
        }
        start = start.saturating_add(step);
    }
    ranges
}

fn stitch_transcripts(parts: &[String]) -> String {
    let mut output = String::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if output.is_empty() {
            output.push_str(part);
            continue;
        }

        let left_words = output.split_whitespace().collect::<Vec<_>>();
        let right_words = part.split_whitespace().collect::<Vec<_>>();
        let max_overlap = left_words
            .len()
            .min(right_words.len())
            .min(MAX_STITCH_OVERLAP_WORDS);
        let overlap = (1..=max_overlap)
            .rev()
            .find(|&count| {
                left_words[left_words.len() - count..]
                    .iter()
                    .zip(&right_words[..count])
                    .all(|(left, right)| normalized_word(left) == normalized_word(right))
            })
            .unwrap_or(0);

        let suffix = right_words[overlap..].join(" ");
        if !suffix.is_empty() {
            if !output.ends_with(char::is_whitespace) {
                output.push(' ');
            }
            output.push_str(&suffix);
        }
    }
    output.trim().to_string()
}

fn normalized_word(word: &str) -> String {
    word.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_canary_audio_stays_one_shot() {
        let sample_rate = 16_000;
        assert_eq!(
            chunk_ranges(37 * sample_rate, sample_rate),
            vec![0..37 * sample_rate]
        );
        assert_eq!(
            chunk_ranges(38 * sample_rate, sample_rate),
            vec![0..38 * sample_rate]
        );
    }

    #[test]
    fn long_canary_audio_is_bounded_with_two_second_overlap() {
        let sample_rate = 16_000;
        let ranges = chunk_ranges(80 * sample_rate, sample_rate);
        assert_eq!(ranges[0], 0..38 * sample_rate);
        assert_eq!(ranges[1], 36 * sample_rate..74 * sample_rate);
        assert_eq!(ranges[2], 72 * sample_rate..80 * sample_rate);
        assert!(ranges
            .iter()
            .all(|range| range.end - range.start <= 38 * sample_rate));
    }

    #[test]
    fn overlapping_chunk_text_is_deduplicated_deterministically() {
        let parts = vec![
            "The quick brown fox jumps over".to_string(),
            "fox jumps over the lazy dog.".to_string(),
            "the lazy dog, and keeps running.".to_string(),
        ];
        assert_eq!(
            stitch_transcripts(&parts),
            "The quick brown fox jumps over the lazy dog. and keeps running."
        );
    }

    #[test]
    fn stitch_keeps_non_overlapping_text() {
        let parts = vec![
            "first sentence.".to_string(),
            "second sentence.".to_string(),
        ];
        assert_eq!(
            stitch_transcripts(&parts),
            "first sentence. second sentence."
        );
    }

    #[test]
    fn cpu_retry_is_reserved_for_gpu_specific_failures() {
        assert!(should_retry_on_cpu(&TranscribeError::Backend("gpu".into())));
        assert!(should_retry_on_cpu(&TranscribeError::OutOfMemory(
            "gpu".into()
        )));
        assert!(!should_retry_on_cpu(&TranscribeError::InvalidArgument(
            "bad input".into()
        )));
        assert!(!should_retry_on_cpu(&TranscribeError::InputTooLong(
            "too long".into()
        )));
        assert!(!should_retry_on_cpu(&TranscribeError::Unsupported(
            "unsupported".into()
        )));
    }

    #[test]
    fn backend_device_metadata_classifies_gpu_without_backend_name_guessing() {
        assert!(device_metadata_is_gpu(DeviceType::Gpu, "vulkan"));
        assert!(device_metadata_is_gpu(DeviceType::Igpu, "vulkan"));
        assert!(device_metadata_is_gpu(DeviceType::Unknown, "metal"));
        assert!(!device_metadata_is_gpu(DeviceType::Cpu, "cpu"));
        assert!(!device_metadata_is_gpu(DeviceType::Accel, "accel"));
    }
}
