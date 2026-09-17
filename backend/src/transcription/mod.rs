mod resampler;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub const WHISPER_SAMPLE_RATE: u32 = 16_000;
pub const DEFAULT_MODEL: &str = "base.en";
const GGML_MAGIC: [u8; 4] = *b"lmgg";

#[derive(Debug, Clone, Copy)]
struct ModelSpec {
    id: &'static str,
    label: &'static str,
    filename: &'static str,
}

const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "tiny.en",
        label: "Tiny English",
        filename: "ggml-tiny.en.bin",
    },
    ModelSpec {
        id: "base.en",
        label: "Base English",
        filename: "ggml-base.en.bin",
    },
    ModelSpec {
        id: "small.en",
        label: "Small English",
        filename: "ggml-small.en.bin",
    },
    ModelSpec {
        id: "medium.en",
        label: "Medium English",
        filename: "ggml-medium.en.bin",
    },
    ModelSpec {
        id: "tiny",
        label: "Tiny Multilingual",
        filename: "ggml-tiny.bin",
    },
    ModelSpec {
        id: "base",
        label: "Base Multilingual",
        filename: "ggml-base.bin",
    },
    ModelSpec {
        id: "small",
        label: "Small Multilingual",
        filename: "ggml-small.bin",
    },
    ModelSpec {
        id: "medium",
        label: "Medium Multilingual",
        filename: "ggml-medium.bin",
    },
    ModelSpec {
        id: "large-v3",
        label: "Large v3 Multilingual",
        filename: "ggml-large-v3.bin",
    },
    ModelSpec {
        id: "large-v3-turbo",
        label: "Large v3 Turbo Multilingual",
        filename: "ggml-large-v3-turbo.bin",
    },
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptionSettings {
    pub model: String,
}

impl Default for TranscriptionSettings {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
        }
    }
}

impl TranscriptionSettings {
    fn path() -> Result<PathBuf, String> {
        Ok(data_root()?.join("transcription_settings.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .ok()
            .and_then(|path| Self::load_from_path(&path).ok())
            .filter(|settings| model_spec(&settings.model).is_some())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        Self::save_to_path(self, &Self::path()?)
    }

    fn load_from_path(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|error| format!("Failed to read transcription settings: {error}"))?;
        serde_json::from_str(&content)
            .map_err(|error| format!("Failed to parse transcription settings: {error}"))
    }

    fn save_to_path(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!("Failed to create transcription settings directory: {error}")
            })?;
        }
        let payload = serde_json::to_string_pretty(self)
            .map_err(|error| format!("Failed to serialize transcription settings: {error}"))?;
        std::fs::write(path, payload)
            .map_err(|error| format!("Failed to write transcription settings: {error}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelInfo {
    pub id: String,
    pub label: String,
    pub filename: String,
    pub path: String,
    pub downloaded: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TranscriptionRuntimePhase {
    Ready,
    ModelMissing,
    Downloading,
    Transcribing,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionRuntimeStatus {
    pub phase: TranscriptionRuntimePhase,
    pub model: String,
    pub model_path: String,
    pub model_downloaded: bool,
    pub last_error: Option<String>,
}

struct LoadedModel {
    id: String,
    context: WhisperContext,
}

#[derive(Clone)]
pub struct TranscriptionService {
    settings: Arc<Mutex<TranscriptionSettings>>,
    runtime: Arc<Mutex<TranscriptionRuntimeStatus>>,
    download_runtime: Arc<Mutex<Option<TranscriptionRuntimeStatus>>>,
    download_guard: Arc<tokio::sync::Mutex<()>>,
    loaded: Arc<Mutex<Option<LoadedModel>>>,
}

impl Default for TranscriptionService {
    fn default() -> Self {
        Self::new()
    }
}

impl TranscriptionService {
    pub fn new() -> Self {
        let settings = TranscriptionSettings::load();
        let status = status_for_model(&settings.model, None);
        Self {
            settings: Arc::new(Mutex::new(settings)),
            runtime: Arc::new(Mutex::new(status)),
            download_runtime: Arc::new(Mutex::new(None)),
            download_guard: Arc::new(tokio::sync::Mutex::new(())),
            loaded: Arc::new(Mutex::new(None)),
        }
    }

    pub fn settings(&self) -> TranscriptionSettings {
        self.settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    pub fn list_models(&self) -> Result<Vec<LocalModelInfo>, String> {
        let selected = self.settings().model;
        let dir = models_dir()?;
        Ok(MODELS
            .iter()
            .map(|spec| model_info(spec, &dir, &selected))
            .collect())
    }

    pub fn runtime_status(&self) -> TranscriptionRuntimeStatus {
        let settings = self.settings();
        let existing = self.runtime.lock().ok().map(|status| status.clone());
        let mut current = status_for_model(
            &settings.model,
            existing
                .as_ref()
                .and_then(|status| status.last_error.clone()),
        );
        if let Some(existing) = existing {
            if existing.model == settings.model
                && matches!(existing.phase, TranscriptionRuntimePhase::Transcribing)
            {
                current.phase = existing.phase;
            } else if existing.model == settings.model
                && matches!(existing.phase, TranscriptionRuntimePhase::Error)
                && existing.last_error.is_some()
            {
                current.phase = TranscriptionRuntimePhase::Error;
            }
        }

        if matches!(
            current.phase,
            TranscriptionRuntimePhase::Transcribing | TranscriptionRuntimePhase::Error
        ) {
            return current;
        }

        if let Ok(download) = self.download_runtime.lock() {
            if let Some(download) = download.as_ref() {
                return download.clone();
            }
        }
        current
    }

    pub fn ensure_ready(&self) -> Result<(), String> {
        let model = self.settings().model;
        let status = status_for_model(&model, None);
        if status.model_downloaded {
            return Ok(());
        }

        let downloading_selected = self
            .download_runtime
            .lock()
            .ok()
            .and_then(|download| download.as_ref().cloned())
            .is_some_and(|download| {
                download.model == model
                    && matches!(download.phase, TranscriptionRuntimePhase::Downloading)
            });
        let action = if downloading_selected {
            "is still downloading"
        } else {
            "is not installed"
        };
        Err(format!(
            "Local transcription model '{}' {action}. Open Settings -> System and download a model before dictating.",
            status.model
        ))
    }

    pub fn set_model(&self, model: &str) -> Result<TranscriptionSettings, String> {
        let normalized = model.trim();
        model_spec(normalized)
            .ok_or_else(|| format!("Unknown local transcription model: {normalized}"))?;

        let settings = TranscriptionSettings {
            model: normalized.to_string(),
        };
        settings.save()?;
        if let Ok(mut current) = self.settings.lock() {
            *current = settings.clone();
        }
        if let Ok(mut loaded) = self.loaded.lock() {
            if loaded.as_ref().map(|loaded| loaded.id.as_str()) != Some(normalized) {
                *loaded = None;
            }
        }
        if let Ok(mut download) = self.download_runtime.lock() {
            if download
                .as_ref()
                .is_some_and(|status| matches!(status.phase, TranscriptionRuntimePhase::Error))
            {
                *download = None;
            }
        }
        self.set_runtime(status_for_model(normalized, None));
        Ok(settings)
    }

    pub async fn download_model(&self, model: &str) -> Result<LocalModelInfo, String> {
        let _download_guard = self.download_guard.lock().await;
        let spec = *model_spec(model.trim())
            .ok_or_else(|| format!("Unknown local transcription model: {}", model.trim()))?;
        let dir = models_dir()?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|error| format!("Failed to create local models directory: {error}"))?;
        let destination = dir.join(spec.filename);
        let selected = self.settings().model;

        if validate_model_file(&destination, None).is_ok() {
            self.set_download_runtime(None);
            return Ok(model_info(&spec, &dir, &selected));
        }

        self.set_download_runtime(Some(TranscriptionRuntimeStatus {
            phase: TranscriptionRuntimePhase::Downloading,
            model: spec.id.to_string(),
            model_path: destination.to_string_lossy().to_string(),
            model_downloaded: false,
            last_error: None,
        }));

        let result = download_model_file(&spec, &destination).await;
        match result {
            Ok(()) => {
                self.clear_download_runtime(spec.id);
                Ok(model_info(&spec, &dir, &selected))
            }
            Err(error) => {
                self.set_download_runtime(Some(TranscriptionRuntimeStatus {
                    phase: TranscriptionRuntimePhase::Error,
                    model: spec.id.to_string(),
                    model_path: destination.to_string_lossy().to_string(),
                    model_downloaded: false,
                    last_error: Some(error.clone()),
                }));
                Err(error)
            }
        }
    }

    pub async fn transcribe(
        &self,
        samples: Vec<f32>,
        sample_rate: u32,
        language: Option<String>,
        dictionary_hints: Vec<String>,
    ) -> Result<String, String> {
        let model = self.settings().model;
        validate_model_language(&model, language.as_deref())?;
        let spec = *model_spec(&model)
            .ok_or_else(|| format!("Unknown local transcription model: {model}"))?;
        let path = models_dir()?.join(spec.filename);
        validate_model_file(&path, None).map_err(|error| {
            format!("Local model '{model}' is not installed or is invalid: {error}. Download it before dictating.")
        })?;

        self.set_runtime(TranscriptionRuntimeStatus {
            phase: TranscriptionRuntimePhase::Transcribing,
            model: model.clone(),
            model_path: path.to_string_lossy().to_string(),
            model_downloaded: true,
            last_error: None,
        });

        let loaded = self.loaded.clone();
        let model_for_task = model.clone();
        let task = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let audio = resampler::resample_buffer(&samples, sample_rate, WHISPER_SAMPLE_RATE)?;
            if audio.is_empty() {
                return Ok(String::new());
            }

            let mut guard = loaded
                .lock()
                .map_err(|_| "Local transcription model lock was poisoned".to_string())?;
            let needs_load =
                guard.as_ref().map(|loaded| loaded.id.as_str()) != Some(model_for_task.as_str());
            if needs_load {
                let mut context_params = WhisperContextParameters::default();
                context_params.use_gpu(false);
                let context = WhisperContext::new_with_params(
                    path.to_str()
                        .ok_or_else(|| "Local model path is not valid UTF-8".to_string())?,
                    context_params,
                )
                .map_err(|error| format!("Failed to load local Whisper model: {error}"))?;
                *guard = Some(LoadedModel {
                    id: model_for_task.clone(),
                    context,
                });
            }

            let loaded = guard.as_ref().expect("loaded model was initialized");
            let mut state = loaded
                .context
                .create_state()
                .map_err(|error| format!("Failed to create Whisper inference state: {error}"))?;
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_n_threads(cpu_thread_count());
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_suppress_blank(true);
            params.set_suppress_nst(true);
            params.set_no_context(true);
            params.set_single_segment(audio.len() < WHISPER_SAMPLE_RATE as usize * 30);

            let selected_language = if model_for_task.ends_with(".en") {
                Some("en".to_string())
            } else {
                language
                    .filter(|value| !value.trim().is_empty() && !value.eq_ignore_ascii_case("auto"))
            };
            params.set_language(selected_language.as_deref());

            let prompt = dictionary_hints
                .iter()
                .map(|hint| hint.trim())
                .filter(|hint| !hint.is_empty())
                .take(20)
                .collect::<Vec<_>>()
                .join(", ");
            if !prompt.is_empty() {
                params.set_initial_prompt(&prompt);
            }

            state
                .full(params, &audio)
                .map_err(|error| format!("Local Whisper inference failed: {error}"))?;

            let mut text = String::new();
            for segment in state.as_iter() {
                text.push_str(
                    segment
                        .to_str()
                        .map_err(|error| format!("Whisper returned invalid text: {error}"))?,
                );
            }
            Ok(text.trim().to_string())
        })
        .await;

        let task = match task {
            Ok(result) => result,
            Err(error) => Err(format!("Local transcription worker failed: {error}")),
        };

        match task {
            Ok(text) => {
                self.set_runtime(status_for_model(&model, None));
                Ok(text)
            }
            Err(error) => {
                let mut status = status_for_model(&model, Some(error.clone()));
                status.phase = TranscriptionRuntimePhase::Error;
                self.set_runtime(status);
                Err(error)
            }
        }
    }

    fn set_runtime(&self, status: TranscriptionRuntimeStatus) {
        if let Ok(mut runtime) = self.runtime.lock() {
            *runtime = status;
        }
    }

    fn set_download_runtime(&self, status: Option<TranscriptionRuntimeStatus>) {
        if let Ok(mut runtime) = self.download_runtime.lock() {
            *runtime = status;
        }
    }

    fn clear_download_runtime(&self, model: &str) {
        if let Ok(mut runtime) = self.download_runtime.lock() {
            if runtime.as_ref().is_some_and(|status| status.model == model) {
                *runtime = None;
            }
        }
    }
}

fn validate_model_language(model: &str, language: Option<&str>) -> Result<(), String> {
    if !model.ends_with(".en") {
        return Ok(());
    }

    let Some(language) = language.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    if language.eq_ignore_ascii_case("auto") || language.to_ascii_lowercase().starts_with("en") {
        return Ok(());
    }

    let multilingual = model.trim_end_matches(".en");
    Err(format!(
        "Local model '{model}' is English-only and cannot transcribe source language '{language}'. Select or download the multilingual '{multilingual}' model in Settings -> System."
    ))
}

fn cpu_thread_count() -> i32 {
    std::thread::available_parallelism()
        .map(|count| count.get().min(4) as i32)
        .unwrap_or(1)
}

fn data_root() -> Result<PathBuf, String> {
    let data_dir =
        dirs_next::data_dir().ok_or_else(|| "Could not find data directory".to_string())?;
    Ok(data_dir.join("ListenOS"))
}

pub fn models_dir() -> Result<PathBuf, String> {
    Ok(data_root()?.join("models"))
}

fn model_spec(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|spec| spec.id == id)
}

fn model_info(spec: &ModelSpec, dir: &Path, selected: &str) -> LocalModelInfo {
    let path = dir.join(spec.filename);
    LocalModelInfo {
        id: spec.id.to_string(),
        label: spec.label.to_string(),
        filename: spec.filename.to_string(),
        path: path.to_string_lossy().to_string(),
        downloaded: validate_model_file(&path, None).is_ok(),
        selected: selected == spec.id,
    }
}

fn status_for_model(model: &str, last_error: Option<String>) -> TranscriptionRuntimeStatus {
    let path =
        model_spec(model).and_then(|spec| models_dir().ok().map(|dir| dir.join(spec.filename)));
    let downloaded = path
        .as_ref()
        .map(|path| validate_model_file(path, None).is_ok())
        .unwrap_or(false);
    TranscriptionRuntimeStatus {
        phase: if downloaded {
            TranscriptionRuntimePhase::Ready
        } else {
            TranscriptionRuntimePhase::ModelMissing
        },
        model: model.to_string(),
        model_path: path
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default(),
        model_downloaded: downloaded,
        last_error,
    }
}

fn model_url(spec: &ModelSpec) -> String {
    format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
        spec.filename
    )
}

// Adapted from VoxType's atomic Whisper model download validation:
// https://github.com/peteonrails/voxtype/blob/320a737/src/setup/model.rs
// Whisper.cpp does not publish a checksum manifest for these files, so the
// response length and ggml container magic are the available integrity checks.
fn validate_model_file(path: &Path, expected_len: Option<u64>) -> Result<(), String> {
    let len = std::fs::metadata(path)
        .map_err(|error| format!("could not stat model: {error}"))?
        .len();
    if len == 0 {
        return Err("model file is empty".to_string());
    }
    if let Some(expected) = expected_len {
        if len != expected {
            return Err(format!("incomplete model: got {len} of {expected} bytes"));
        }
    }

    let mut magic = [0_u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .map_err(|error| format!("could not read model header: {error}"))?;
    if magic != GGML_MAGIC {
        return Err(format!(
            "not a ggml model: expected magic {GGML_MAGIC:02x?}, got {magic:02x?}"
        ));
    }
    Ok(())
}

async fn download_model_file(spec: &ModelSpec, destination: &Path) -> Result<(), String> {
    let url = model_url(spec);
    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|error| format!("Failed to download local model '{}': {error}", spec.id))?;
    if !response.status().is_success() {
        return Err(format!(
            "Model download failed with HTTP {}",
            response.status()
        ));
    }
    let expected_len = response.content_length();
    let part = destination.with_extension("bin.part");
    let _ = tokio::fs::remove_file(&part).await;
    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|error| format!("Failed to create model download file: {error}"))?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("Model download interrupted: {error}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("Failed to write model download: {error}"))?;
    }
    file.flush()
        .await
        .map_err(|error| format!("Failed to flush model download: {error}"))?;
    drop(file);

    if let Err(error) = validate_model_file(&part, expected_len) {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(format!(
            "Downloaded model '{}' is invalid: {error}",
            spec.id
        ));
    }

    if destination.exists() {
        tokio::fs::remove_file(destination)
            .await
            .map_err(|error| format!("Failed to replace invalid local model: {error}"))?;
    }
    tokio::fs::rename(&part, destination)
        .await
        .map_err(|error| format!("Failed to install downloaded local model: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("listenos-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn default_model_is_base_english() {
        assert_eq!(TranscriptionSettings::default().model, "base.en");
        let spec = model_spec(DEFAULT_MODEL).expect("default model must be in catalog");
        assert_eq!(spec.filename, "ggml-base.en.bin");
    }

    #[test]
    fn transcription_settings_round_trip() {
        let path = temp_file("transcription-settings.json");
        let settings = TranscriptionSettings {
            model: "small.en".to_string(),
        };
        settings.save_to_path(&path).expect("save settings");
        assert_eq!(
            TranscriptionSettings::load_from_path(&path).unwrap(),
            settings
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ggml_validation_rejects_truncated_and_non_model_files() {
        let valid = temp_file("valid-model.bin");
        std::fs::write(&valid, [b'l', b'm', b'g', b'g', 1, 2, 3, 4]).unwrap();
        validate_model_file(&valid, Some(8)).expect("valid model header");
        assert!(validate_model_file(&valid, Some(9)).is_err());

        let html = temp_file("html-model.bin");
        std::fs::write(&html, b"<html>error</html>").unwrap();
        assert!(validate_model_file(&html, None).is_err());

        let _ = std::fs::remove_file(valid);
        let _ = std::fs::remove_file(html);
    }

    #[test]
    fn model_urls_point_to_whisper_cpp_artifacts() {
        let spec = model_spec("base.en").unwrap();
        let url = model_url(spec);
        assert!(url.ends_with("/ggml-base.en.bin"));
        assert!(url.contains("huggingface.co/ggerganov/whisper.cpp"));
    }

    #[test]
    fn english_only_models_reject_non_english_language_hints() {
        assert!(validate_model_language("base.en", Some("en")).is_ok());
        assert!(validate_model_language("base.en", Some("en-US")).is_ok());
        assert!(validate_model_language("base.en", Some("auto")).is_ok());
        assert!(validate_model_language("base", Some("hi")).is_ok());

        let error = validate_model_language("base.en", Some("hi"))
            .expect_err("English-only model must reject Hindi");
        assert!(error.contains("English-only"));
        assert!(error.contains("multilingual 'base'"));
    }

    #[test]
    fn download_status_names_the_downloaded_model_without_overwriting_transcription() {
        let selected = "base.en";
        let service = TranscriptionService {
            settings: Arc::new(Mutex::new(TranscriptionSettings {
                model: selected.to_string(),
            })),
            runtime: Arc::new(Mutex::new(status_for_model(selected, None))),
            download_runtime: Arc::new(Mutex::new(Some(TranscriptionRuntimeStatus {
                phase: TranscriptionRuntimePhase::Downloading,
                model: "small.en".to_string(),
                model_path: "small.en.part".to_string(),
                model_downloaded: false,
                last_error: None,
            }))),
            download_guard: Arc::new(tokio::sync::Mutex::new(())),
            loaded: Arc::new(Mutex::new(None)),
        };

        let downloading = service.runtime_status();
        assert_eq!(downloading.phase, TranscriptionRuntimePhase::Downloading);
        assert_eq!(downloading.model, "small.en");
        assert!(!downloading.model_downloaded);

        service.set_runtime(TranscriptionRuntimeStatus {
            phase: TranscriptionRuntimePhase::Transcribing,
            model: selected.to_string(),
            model_path: "base.en".to_string(),
            model_downloaded: true,
            last_error: None,
        });
        let transcribing = service.runtime_status();
        assert_eq!(transcribing.phase, TranscriptionRuntimePhase::Transcribing);
        assert_eq!(transcribing.model, selected);
    }
}
