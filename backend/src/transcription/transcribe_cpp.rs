use std::ops::Range;
use std::path::Path;

use transcribe_cpp::{
    Backend, Capabilities, DeviceType, Error as TranscribeError, Model, ModelOptions, RunOptions,
    Session, SessionOptions, TimestampKind,
};

use super::{BackendSelection, TranscriptionComputeBackend, WHISPER_SAMPLE_RATE};

const CANARY_MAX_CHUNK_MS: usize = 38_000;
const QWEN_MAX_CHUNK_CAP_MS: usize = 30_000;
const QWEN_LIMIT_SAFETY_MS: usize = 500;
const CHUNK_OVERLAP_MS: usize = 2_000;
const MAX_STITCH_OVERLAP_WORDS: usize = 24;
const MAX_INPUT_TOO_LONG_SPLITS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Family {
    CanaryQwen,
    Qwen3Asr,
}

impl Family {
    pub(super) fn expected_arch(self) -> &'static str {
        match self {
            Self::CanaryQwen => "canary_qwen",
            Self::Qwen3Asr => "qwen3_asr",
        }
    }

    pub(super) fn display_name(self) -> &'static str {
        match self {
            Self::CanaryQwen => "Canary-Qwen",
            Self::Qwen3Asr => "Qwen3-ASR",
        }
    }

    pub(super) fn repository(self) -> &'static str {
        match self {
            Self::CanaryQwen => "handy-computer/canary-qwen-2.5b-gguf",
            Self::Qwen3Asr => "handy-computer/Qwen3-ASR-1.7B-gguf",
        }
    }

    fn supports_language_hint(self) -> bool {
        matches!(self, Self::Qwen3Asr)
    }

    fn maximum_chunk_ms(self, capabilities: &Capabilities) -> usize {
        match self {
            Self::CanaryQwen => CANARY_MAX_CHUNK_MS,
            Self::Qwen3Asr => {
                let advertised = usize::try_from(capabilities.max_audio_ms).unwrap_or(0);
                if advertised == 0 {
                    QWEN_MAX_CHUNK_CAP_MS
                } else {
                    advertised
                        .saturating_sub(QWEN_LIMIT_SAFETY_MS)
                        .min(QWEN_MAX_CHUNK_CAP_MS)
                        .max(CHUNK_OVERLAP_MS + 1)
                }
            }
        }
    }
}

pub(super) struct Runtime {
    family: Family,
    session: Session,
    capabilities: Capabilities,
}

impl Runtime {
    pub(super) fn load(
        path: &Path,
        gpu_requested: bool,
        family: Family,
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
        .map_err(|error| format!("Failed to load {} model: {error}", family.display_name()))?;

        let architecture = model.arch();
        validate_architecture(family, &architecture)?;
        let capabilities = model.capabilities();
        let backend = loaded_backend(&model, gpu_requested, family);
        let session = model
            .session_with(&SessionOptions {
                n_threads: super::cpu_thread_count(),
                ..SessionOptions::default()
            })
            .map_err(|error| {
                format!(
                    "Failed to create {} inference session: {error}",
                    family.display_name()
                )
            })?;
        Ok((
            Self {
                family,
                session,
                capabilities,
            },
            backend,
        ))
    }

    pub(super) fn transcribe(
        &mut self,
        audio: &[f32],
        language: Option<&str>,
        dictionary_hints: &[(String, Option<String>)],
    ) -> Result<String, TranscribeError> {
        if !dictionary_hints.is_empty() {
            log::debug!(
                "{} does not expose an initial-prompt path; {} recognition dictionary hints are not passed to this model",
                self.family.display_name(),
                dictionary_hints.len()
            );
        }

        let language = selected_language(self.family, &self.capabilities, language);
        let options = RunOptions {
            timestamps: TimestampKind::None,
            language,
            ..RunOptions::default()
        };
        let max_chunk_ms = self.family.maximum_chunk_ms(&self.capabilities);
        let ranges = chunk_ranges(
            audio.len(),
            WHISPER_SAMPLE_RATE as usize,
            max_chunk_ms,
            CHUNK_OVERLAP_MS,
        );
        let mut transcripts = Vec::with_capacity(ranges.len());
        for range in ranges {
            self.run_chunk_with_retry(&audio[range], &options, 0, &mut transcripts)?;
        }
        let sanitized = transcripts
            .iter()
            .map(|part| sanitize_output(part))
            .collect::<Vec<_>>();
        Ok(stitch_transcripts(&sanitized))
    }

    fn run_chunk_with_retry(
        &mut self,
        audio: &[f32],
        options: &RunOptions,
        depth: usize,
        output: &mut Vec<String>,
    ) -> Result<(), TranscribeError> {
        match self.session.run(audio, options) {
            Ok(transcript) => {
                output.push(transcript.text);
                Ok(())
            }
            Err(TranscribeError::InputTooLong(_))
                if self.family == Family::Qwen3Asr
                    && depth < MAX_INPUT_TOO_LONG_SPLITS
                    && audio.len() > WHISPER_SAMPLE_RATE as usize * 2 =>
            {
                let overlap = WHISPER_SAMPLE_RATE as usize;
                let midpoint = audio.len() / 2;
                let left_end = midpoint.saturating_add(overlap).min(audio.len());
                let right_start = midpoint.saturating_sub(overlap);
                self.run_chunk_with_retry(&audio[..left_end], options, depth + 1, output)?;
                self.run_chunk_with_retry(&audio[right_start..], options, depth + 1, output)
            }
            Err(error) => Err(error),
        }
    }
}

fn validate_architecture(family: Family, architecture: &str) -> Result<(), String> {
    if architecture == family.expected_arch() {
        Ok(())
    } else {
        Err(format!(
            "{} artifact has unexpected architecture '{architecture}' (expected '{}')",
            family.display_name(),
            family.expected_arch()
        ))
    }
}

fn selected_language(
    family: Family,
    capabilities: &Capabilities,
    language: Option<&str>,
) -> Option<String> {
    if !family.supports_language_hint() {
        return None;
    }
    let language = language?.trim().to_ascii_lowercase();
    if language.is_empty() || language == "auto" {
        return None;
    }
    let language = language.split('-').next().unwrap_or(&language).to_string();
    if capabilities
        .languages
        .iter()
        .any(|supported| supported.eq_ignore_ascii_case(&language))
    {
        Some(language)
    } else {
        log::warn!(
            "{} does not advertise language '{}'; falling back to automatic language detection",
            family.display_name(),
            language
        );
        None
    }
}

pub(super) fn should_retry_on_cpu(error: &TranscribeError) -> bool {
    matches!(
        error,
        TranscribeError::Backend(_) | TranscribeError::OutOfMemory(_)
    )
}

pub(super) fn planned_backend(gpu_requested: bool, family: Family) -> BackendSelection {
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
            accelerator: Some(device_label(
                &device.kind,
                &device.description,
                &device.name,
            )),
            fallback_reason: None,
        },
        None => BackendSelection {
            active: TranscriptionComputeBackend::Cpu,
            accelerator: None,
            fallback_reason: Some(format!(
                "GPU is enabled, but the {} runtime found no usable GPU device. Using CPU.",
                family.display_name()
            )),
        },
    }
}

fn loaded_backend(model: &Model, gpu_requested: bool, family: Family) -> BackendSelection {
    let backend_name = model.backend();
    let device = model.device().ok();
    let is_gpu = device
        .as_ref()
        .is_some_and(|device| device_metadata_is_gpu(device.device_type, &device.kind));
    if is_gpu {
        BackendSelection {
            active: TranscriptionComputeBackend::Gpu,
            accelerator: device
                .map(|device| device_label(&backend_name, &device.description, &device.name))
                .or_else(|| Some(backend_name.clone())),
            fallback_reason: None,
        }
    } else {
        BackendSelection {
            active: TranscriptionComputeBackend::Cpu,
            accelerator: None,
            fallback_reason: gpu_requested.then(|| {
                format!(
                    "GPU was requested for {}, but the runtime selected '{backend_name}'. Using CPU.",
                    family.display_name()
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

fn chunk_ranges(
    sample_count: usize,
    sample_rate: usize,
    max_chunk_ms: usize,
    overlap_ms: usize,
) -> Vec<Range<usize>> {
    if sample_count == 0 || sample_rate == 0 || max_chunk_ms == 0 {
        return Vec::new();
    }
    let max_samples = sample_rate.saturating_mul(max_chunk_ms) / 1000;
    if sample_count <= max_samples {
        return vec![0..sample_count];
    }
    let overlap = (sample_rate.saturating_mul(overlap_ms) / 1000).min(max_samples - 1);
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
        let mut suffix = right_words[overlap..].join(" ");
        if output.ends_with(['.', '!', '?']) {
            uppercase_first_letter(&mut suffix);
        }
        if !suffix.is_empty() {
            if !output.ends_with(char::is_whitespace) {
                output.push(' ');
            }
            output.push_str(&suffix);
        }
    }
    output.trim().to_string()
}

fn uppercase_first_letter(text: &mut String) {
    let Some((index, character)) = text
        .char_indices()
        .find(|(_, character)| character.is_alphabetic())
    else {
        return;
    };
    let end = index + character.len_utf8();
    text.replace_range(index..end, &character.to_uppercase().collect::<String>());
}

fn normalized_word(word: &str) -> String {
    word.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn sanitize_output(text: &str) -> String {
    let mut cleaned = text
        .replace("<asr_text>", "")
        .replace("</asr_text>", "")
        .replace("<|im_end|>", "")
        .replace("<|im_start|>", "");
    cleaned = cleaned
        .lines()
        .filter(|line| !line.trim().to_ascii_lowercase().starts_with("language "))
        .collect::<Vec<_>>()
        .join(" ");
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canary_chunk_plan_is_unchanged() {
        let rate = 16_000;
        let ranges = chunk_ranges(80 * rate, rate, CANARY_MAX_CHUNK_MS, CHUNK_OVERLAP_MS);
        assert_eq!(
            ranges,
            vec![0..38 * rate, 36 * rate..74 * rate, 72 * rate..80 * rate]
        );
    }

    #[test]
    fn qwen_chunk_plan_uses_capability_limit_with_safety_and_cap() {
        let rate = 16_000;
        let capabilities = Capabilities {
            native_sample_rate: rate as i32,
            languages: vec!["en".into()],
            translate_target_languages: Vec::new(),
            max_timestamp_kind: TimestampKind::None,
            supports_language_detect: true,
            supports_translate: false,
            supports_streaming: false,
            supports_spec_decode: false,
            max_audio_ms: 25_000,
        };
        let max = Family::Qwen3Asr.maximum_chunk_ms(&capabilities);
        assert_eq!(max, 24_500);
        let ranges = chunk_ranges(50 * rate, rate, max, CHUNK_OVERLAP_MS);
        assert!(ranges
            .iter()
            .all(|range| range.len() <= 24_500 * rate / 1000));
        assert_eq!(ranges[1].start, ranges[0].end - 2 * rate);
    }

    #[test]
    fn stitch_capitalizes_fragment_after_sentence_boundary() {
        let parts = vec![
            "The quick brown fox jumps over".to_string(),
            "fox jumps over the lazy dog.".to_string(),
            "the lazy dog, and keeps running.".to_string(),
        ];
        assert_eq!(
            stitch_transcripts(&parts),
            "The quick brown fox jumps over the lazy dog. And keeps running."
        );
    }

    #[test]
    fn qwen_output_sanitation_removes_chat_template_residue() {
        assert_eq!(
            sanitize_output("language English\n<asr_text>Hello there.</asr_text><|im_end|>"),
            "Hello there."
        );
    }

    #[test]
    fn family_architecture_mismatch_names_expected_architecture() {
        let error = validate_architecture(Family::Qwen3Asr, "canary_qwen").unwrap_err();
        assert!(error.contains("Qwen3-ASR"));
        assert!(error.contains("qwen3_asr"));
    }

    #[test]
    fn cpu_retry_is_reserved_for_gpu_specific_failures() {
        assert!(should_retry_on_cpu(&TranscribeError::Backend("gpu".into())));
        assert!(should_retry_on_cpu(&TranscribeError::OutOfMemory(
            "gpu".into()
        )));
        assert!(!should_retry_on_cpu(&TranscribeError::InputTooLong(
            "long".into()
        )));
    }
}
