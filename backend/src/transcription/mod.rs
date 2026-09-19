mod resampler;

use futures_util::StreamExt;
use reqwest::{
    header::{CONTENT_RANGE, RANGE},
    StatusCode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Once, OnceLock,
};
use std::time::{Instant, SystemTime};
use tokio::io::AsyncWriteExt;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub const WHISPER_SAMPLE_RATE: u32 = 16_000;
pub const DEFAULT_MODEL: &str = "base.en";
const GGML_MAGIC: [u8; 4] = *b"lmgg";
const MIN_MODEL_FILE_BYTES: u64 = 1024 * 1024;
const MAX_MODEL_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifiedModelIdentity {
    len: u64,
    modified: Option<SystemTime>,
    expected_sha256: &'static str,
}

static VERIFIED_MODELS: OnceLock<Mutex<HashMap<PathBuf, VerifiedModelIdentity>>> = OnceLock::new();

#[cfg(test)]
static SHA256_FILE_CALLS: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
struct ModelSpec {
    id: &'static str,
    label: &'static str,
    filename: &'static str,
    revision: &'static str,
    sha256: &'static str,
    tier: &'static str,
    english_only: bool,
    recommended: bool,
}

const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "tiny.en",
        label: "Tiny English",
        filename: "ggml-tiny.en.bin",
        revision: "80da2d8",
        sha256: "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f",
        tier: "tiny",
        english_only: true,
        recommended: false,
    },
    ModelSpec {
        id: "base.en",
        label: "Base English",
        filename: "ggml-base.en.bin",
        revision: "80da2d8",
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
        tier: "base",
        english_only: true,
        recommended: true,
    },
    ModelSpec {
        id: "small.en",
        label: "Small English",
        filename: "ggml-small.en.bin",
        revision: "80da2d8",
        sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
        tier: "small",
        english_only: true,
        recommended: false,
    },
    ModelSpec {
        id: "medium.en",
        label: "Medium English",
        filename: "ggml-medium.en.bin",
        revision: "80da2d8",
        sha256: "cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356",
        tier: "medium",
        english_only: true,
        recommended: false,
    },
    ModelSpec {
        id: "tiny",
        label: "Tiny Multilingual",
        filename: "ggml-tiny.bin",
        revision: "80da2d8",
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        tier: "tiny",
        english_only: false,
        recommended: false,
    },
    ModelSpec {
        id: "base",
        label: "Base Multilingual",
        filename: "ggml-base.bin",
        revision: "80da2d8",
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        tier: "base",
        english_only: false,
        recommended: false,
    },
    ModelSpec {
        id: "small",
        label: "Small Multilingual",
        filename: "ggml-small.bin",
        revision: "80da2d8",
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        tier: "small",
        english_only: false,
        recommended: false,
    },
    ModelSpec {
        id: "medium",
        label: "Medium Multilingual",
        filename: "ggml-medium.bin",
        revision: "80da2d8",
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        tier: "medium",
        english_only: false,
        recommended: false,
    },
    ModelSpec {
        id: "large-v3",
        label: "Large v3 Multilingual",
        filename: "ggml-large-v3.bin",
        revision: "362722b",
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
        tier: "large-v3",
        english_only: false,
        recommended: false,
    },
    ModelSpec {
        id: "large-v3-turbo",
        label: "Large v3 Turbo Multilingual",
        filename: "ggml-large-v3-turbo.bin",
        revision: "98aa99a",
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        tier: "large-v3-turbo",
        english_only: false,
        recommended: false,
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
        Ok(crate::app_data_root()?.join("transcription_settings.json"))
    }

    pub fn load() -> Self {
        let Some(active_path) = Self::path().ok() else {
            return Self::default();
        };
        if crate::paths::path_exists_no_follow(&active_path) {
            return if crate::paths::regular_file_exists_no_follow(&active_path) {
                Self::load_from_path(&active_path)
                    .ok()
                    .filter(|settings| model_spec(&settings.model).is_some())
                    .unwrap_or_default()
            } else {
                Self::default()
            };
        }

        crate::paths::app_data_file_candidates("transcription_settings.json")
            .unwrap_or_default()
            .into_iter()
            .skip(1)
            .find_map(|path| {
                if !crate::paths::regular_file_exists_no_follow(&path) {
                    return None;
                }
                let settings = Self::load_from_path(&path)
                    .ok()
                    .filter(|settings| model_spec(&settings.model).is_some())?;
                let _ = settings.save();
                Some(settings)
            })
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
    pub revision: String,
    pub sha256: String,
    pub tier: String,
    pub english_only: bool,
    pub recommended: bool,
    pub path: String,
    pub downloaded: bool,
    pub installed_bytes: Option<u64>,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TranscriptionRuntimePhase {
    Ready,
    ModelMissing,
    Downloading,
    Loading,
    Transcribing,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TranscriptionComputeBackend {
    Cpu,
    Gpu,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionRuntimeStatus {
    pub phase: TranscriptionRuntimePhase,
    pub model: String,
    pub model_path: String,
    pub model_downloaded: bool,
    #[serde(default)]
    pub download_bytes: Option<u64>,
    #[serde(default)]
    pub download_total_bytes: Option<u64>,
    /// The persisted user preference. This can be true while `active_backend`
    /// is CPU when the shipped build or current machine cannot initialize a
    /// supported accelerator.
    #[serde(default = "default_gpu_requested")]
    pub gpu_requested: bool,
    #[serde(default)]
    pub active_backend: Option<TranscriptionComputeBackend>,
    #[serde(default)]
    pub accelerator: Option<String>,
    #[serde(default)]
    pub backend_fallback_reason: Option<String>,
    pub last_error: Option<String>,
}

struct LoadedModel {
    id: String,
    gpu_requested: bool,
    backend: BackendSelection,
    context: WhisperContext,
}

#[derive(Debug, Clone)]
struct BackendSelection {
    active: TranscriptionComputeBackend,
    accelerator: Option<String>,
    fallback_reason: Option<String>,
}

#[derive(Debug)]
enum WhisperInferenceError {
    State(String),
    Full(String),
    Output(String),
}

impl WhisperInferenceError {
    fn into_message(self) -> String {
        match self {
            Self::State(error) => format!("Failed to create Whisper inference state: {error}"),
            Self::Full(error) => format!("Local Whisper inference failed: {error}"),
            Self::Output(error) => format!("Whisper returned invalid text: {error}"),
        }
    }

    fn full_error(&self) -> Option<&str> {
        match self {
            Self::Full(error) => Some(error),
            Self::State(_) | Self::Output(_) => None,
        }
    }
}

#[derive(Clone)]
pub struct TranscriptionService {
    settings: Arc<Mutex<TranscriptionSettings>>,
    runtime: Arc<Mutex<TranscriptionRuntimeStatus>>,
    download_runtime: Arc<Mutex<Option<TranscriptionRuntimeStatus>>>,
    download_guard: Arc<tokio::sync::Mutex<()>>,
    loaded: Arc<Mutex<Option<LoadedModel>>>,
    gpu_enabled: Arc<AtomicBool>,
}

impl Default for TranscriptionService {
    fn default() -> Self {
        Self::new()
    }
}

impl TranscriptionService {
    pub fn new() -> Self {
        Self::new_with_gpu_enabled(true)
    }

    pub fn new_with_gpu_enabled(gpu_enabled: bool) -> Self {
        let settings = TranscriptionSettings::load();
        let status = status_for_model(&settings.model, None, gpu_enabled, None);
        Self {
            settings: Arc::new(Mutex::new(settings)),
            runtime: Arc::new(Mutex::new(status)),
            download_runtime: Arc::new(Mutex::new(None)),
            download_guard: Arc::new(tokio::sync::Mutex::new(())),
            loaded: Arc::new(Mutex::new(None)),
            gpu_enabled: Arc::new(AtomicBool::new(gpu_enabled)),
        }
    }

    pub fn gpu_enabled(&self) -> bool {
        self.gpu_enabled.load(Ordering::Acquire)
    }

    /// Change the requested execution device and invalidate any context loaded
    /// under the previous policy. The caller should preload the selected model
    /// after persisting the setting so the next dictation does not pay reload
    /// latency.
    pub fn set_gpu_enabled(&self, enabled: bool) {
        let previous = self.gpu_enabled.swap(enabled, Ordering::AcqRel);
        if previous != enabled {
            if let Ok(mut loaded) = self.loaded.lock() {
                *loaded = None;
            }
        }
        let model = self.settings().model;
        self.set_runtime(status_for_model(&model, None, enabled, None));
    }

    pub fn settings(&self) -> TranscriptionSettings {
        self.settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    pub fn list_models(&self) -> Result<Vec<LocalModelInfo>, String> {
        let selected = self.settings().model;
        Ok(MODELS
            .iter()
            .map(|spec| model_info(spec, &selected))
            .collect())
    }

    pub fn runtime_status(&self) -> TranscriptionRuntimeStatus {
        let settings = self.settings();
        let existing = self.runtime.lock().ok().map(|status| status.clone());
        let mut current = self.status_for_model(
            &settings.model,
            existing
                .as_ref()
                .and_then(|status| status.last_error.clone()),
        );
        if let Some(existing) = existing {
            if existing.model == settings.model
                && matches!(
                    existing.phase,
                    TranscriptionRuntimePhase::Loading | TranscriptionRuntimePhase::Transcribing
                )
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
            TranscriptionRuntimePhase::Loading
                | TranscriptionRuntimePhase::Transcribing
                | TranscriptionRuntimePhase::Error
        ) {
            return current;
        }

        if let Ok(download) = self.download_runtime.lock() {
            if let Some(download) = download.as_ref() {
                let mut download = download.clone();
                download.gpu_requested = current.gpu_requested;
                download.active_backend = current.active_backend;
                download.accelerator = current.accelerator;
                download.backend_fallback_reason = current.backend_fallback_reason;
                return download;
            }
        }
        current
    }

    pub fn ensure_ready(&self) -> Result<(), String> {
        let model = self.settings().model;
        let status = self.status_for_model(&model, None);
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
            "Local transcription model '{}' {action}. Run `origin model download {}` and `origin model use {}` before dictating.",
            status.model, status.model, status.model
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
        self.set_runtime(self.status_for_model(normalized, None));
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

        if find_valid_model_path(&spec)?.is_some() {
            self.set_download_runtime(None);
            return Ok(model_info(&spec, &selected));
        }

        self.set_download_runtime(Some(TranscriptionRuntimeStatus {
            phase: TranscriptionRuntimePhase::Downloading,
            model: spec.id.to_string(),
            model_path: destination.to_string_lossy().to_string(),
            model_downloaded: false,
            download_bytes: Some(0),
            download_total_bytes: None,
            gpu_requested: self.gpu_enabled(),
            active_backend: None,
            accelerator: None,
            backend_fallback_reason: None,
            last_error: None,
        }));

        let download_runtime = self.download_runtime.clone();
        let model_id = spec.id;
        let result = download_model_file(&spec, &destination, move |downloaded, total| {
            if let Ok(mut runtime) = download_runtime.lock() {
                let Some(status) = runtime.as_mut() else {
                    return;
                };
                if status.model != model_id {
                    return;
                }
                status.download_bytes = Some(downloaded);
                status.download_total_bytes = total;
            }
        })
        .await;
        match result {
            Ok(()) => {
                self.clear_download_runtime(spec.id);
                Ok(model_info(&spec, &selected))
            }
            Err(error) => {
                self.set_download_runtime(Some(TranscriptionRuntimeStatus {
                    phase: TranscriptionRuntimePhase::Error,
                    model: spec.id.to_string(),
                    model_path: destination.to_string_lossy().to_string(),
                    model_downloaded: false,
                    download_bytes: None,
                    download_total_bytes: None,
                    gpu_requested: self.gpu_enabled(),
                    active_backend: None,
                    accelerator: None,
                    backend_fallback_reason: None,
                    last_error: Some(error.clone()),
                }));
                Err(error)
            }
        }
    }

    /// Remove one catalog model from every backend-owned storage root.
    /// Only the exact catalog filename (and its `.part` download) is touched.
    /// Symlinks/reparse points are removed as leaf entries and are never followed.
    pub async fn remove_model(&self, model: &str) -> Result<bool, String> {
        let _download_guard = self.download_guard.lock().await;
        let normalized = model.trim();
        let spec = *model_spec(normalized)
            .ok_or_else(|| format!("Unknown local transcription model: {normalized}"))?;
        let roots = crate::model_storage_roots()?;
        let removed_any = remove_model_from_roots(&spec, &roots)?;
        if removed_any {
            if let Ok(mut loaded) = self.loaded.lock() {
                if loaded.as_ref().is_some_and(|loaded| loaded.id == spec.id) {
                    *loaded = None;
                }
            }
            self.set_runtime(self.status_for_model(normalized, None));
        }
        self.clear_download_runtime(spec.id);
        Ok(removed_any)
    }

    /// Load the selected local Whisper model into memory without running inference.
    /// This is safe to call repeatedly and is intentionally run off the UI thread so
    /// the first short dictation does not pay the full model-open cost after release.
    pub async fn preload_selected_model(&self) -> Result<(), String> {
        let model = self.settings().model;
        let spec = *model_spec(&model)
            .ok_or_else(|| format!("Unknown local transcription model: {model}"))?;
        let path = find_valid_model_path(&spec)?.ok_or_else(|| {
            format!("Local model '{model}' is not installed or is invalid. Download it before dictating.")
        })?;

        let mut loading_status = self.status_for_model(&model, None);
        loading_status.phase = TranscriptionRuntimePhase::Loading;
        self.set_runtime(TranscriptionRuntimeStatus {
            phase: TranscriptionRuntimePhase::Loading,
            model: model.clone(),
            model_path: path.to_string_lossy().to_string(),
            model_downloaded: true,
            download_bytes: None,
            download_total_bytes: None,
            ..loading_status
        });

        let loaded = self.loaded.clone();
        let model_for_task = model.clone();
        let gpu_requested = self.gpu_enabled();
        let started = Instant::now();
        let result = tokio::task::spawn_blocking(move || {
            ensure_model_loaded(&loaded, &model_for_task, &path, gpu_requested)
        })
        .await
        .map_err(|error| format!("Local transcription model preload worker failed: {error}"))?;
        if let Err(error) = result {
            let mut status = self.status_for_model(&model, Some(error.clone()));
            status.phase = TranscriptionRuntimePhase::Error;
            self.set_runtime(status);
            return Err(error);
        }
        self.set_runtime(self.status_for_model(&model, None));
        log::info!(
            "Local transcription model '{}' ready in {} ms",
            model,
            started.elapsed().as_millis()
        );
        #[cfg(debug_assertions)]
        eprintln!(
            "[Origin Speak timing] model_preload model={} elapsed_ms={}",
            model,
            started.elapsed().as_millis()
        );
        Ok(())
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
        let path = find_valid_model_path(&spec)?.ok_or_else(|| {
            format!("Local model '{model}' is not installed or is invalid. Download it before dictating.")
        })?;

        let mut transcribing_status = self.status_for_model(&model, None);
        transcribing_status.phase = TranscriptionRuntimePhase::Transcribing;
        self.set_runtime(TranscriptionRuntimeStatus {
            phase: TranscriptionRuntimePhase::Transcribing,
            model: model.clone(),
            model_path: path.to_string_lossy().to_string(),
            model_downloaded: true,
            download_bytes: None,
            download_total_bytes: None,
            ..transcribing_status
        });

        let loaded = self.loaded.clone();
        let model_for_task = model.clone();
        let gpu_requested = self.gpu_enabled();
        let task = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let total_started = Instant::now();
            let resample_started = Instant::now();
            let audio = resampler::resample_buffer(&samples, sample_rate, WHISPER_SAMPLE_RATE)?;
            let resample_ms = resample_started.elapsed().as_millis();
            if audio.is_empty() {
                return Ok(String::new());
            }

            let load_started = Instant::now();
            let mut guard = loaded
                .lock()
                .map_err(|_| "Local transcription model lock was poisoned".to_string())?;
            let needs_load = guard.as_ref().is_none_or(|loaded| {
                loaded.id != model_for_task || loaded.gpu_requested != gpu_requested
            });
            if needs_load {
                load_model(&mut guard, &model_for_task, &path, gpu_requested)?;
            }
            let load_ms = load_started.elapsed().as_millis();

            let inference_started = Instant::now();
            let initial_backend = guard
                .as_ref()
                .expect("loaded model was initialized")
                .backend
                .clone();
            let initial_result = {
                let loaded = guard.as_ref().expect("loaded model was initialized");
                run_whisper_inference(
                    &loaded.context,
                    &audio,
                    &model_for_task,
                    language.as_deref(),
                    &dictionary_hints,
                )
            };
            let text = match initial_result {
                Ok(text) => text,
                Err(error) if should_retry_inference_on_cpu(&initial_backend, &error) => {
                    let gpu_error = error
                        .full_error()
                        .expect("GPU inference fallback only handles full() failures")
                        .to_string();
                    log::warn!(
                        "GPU Whisper inference failed for '{}': {}. Reloading on CPU and retrying once.",
                        model_for_task,
                        gpu_error
                    );
                    load_cpu_after_gpu_inference_failure(
                        &mut guard,
                        &model_for_task,
                        &path,
                        gpu_requested,
                        &gpu_error,
                    )?;
                    let loaded = guard.as_ref().expect("CPU fallback model was initialized");
                    run_whisper_inference(
                        &loaded.context,
                        &audio,
                        &model_for_task,
                        language.as_deref(),
                        &dictionary_hints,
                    )
                    .map_err(|cpu_error| {
                        format!(
                            "GPU inference failed ({gpu_error}); CPU fallback retry failed: {}",
                            cpu_error.into_message()
                        )
                    })?
                }
                Err(error) => return Err(error.into_message()),
            };
            let inference_ms = inference_started.elapsed().as_millis();
            log::info!(
                "Local transcription timing: model={}, audio_ms={}, resample_ms={}, model_load_ms={}, inference_ms={}, total_ms={}, text_chars={}",
                model_for_task,
                (audio.len() as u64 * 1000) / WHISPER_SAMPLE_RATE as u64,
                resample_ms,
                load_ms,
                inference_ms,
                total_started.elapsed().as_millis(),
                text.chars().count()
            );
            #[cfg(debug_assertions)]
            eprintln!(
                "[Origin Speak timing] transcription model={} audio_ms={} resample_ms={} model_load_ms={} inference_ms={} total_ms={} text_chars={}",
                model_for_task,
                (audio.len() as u64 * 1000) / WHISPER_SAMPLE_RATE as u64,
                resample_ms,
                load_ms,
                inference_ms,
                total_started.elapsed().as_millis(),
                text.chars().count()
            );
            Ok(text)
        })
        .await;

        let task = match task {
            Ok(result) => result,
            Err(error) => Err(format!("Local transcription worker failed: {error}")),
        };

        match task {
            Ok(text) => {
                self.set_runtime(self.status_for_model(&model, None));
                Ok(text)
            }
            Err(error) => {
                let mut status = self.status_for_model(&model, Some(error.clone()));
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

    fn status_for_model(
        &self,
        model: &str,
        last_error: Option<String>,
    ) -> TranscriptionRuntimeStatus {
        let gpu_requested = self.gpu_enabled();
        let backend = self.loaded.lock().ok().and_then(|loaded| {
            loaded.as_ref().and_then(|loaded| {
                (loaded.id == model && loaded.gpu_requested == gpu_requested)
                    .then(|| loaded.backend.clone())
            })
        });
        status_for_model(model, last_error, gpu_requested, backend.as_ref())
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
        "Local model '{model}' is English-only and cannot transcribe source language '{language}'. Run `origin model download {multilingual}` and `origin model use {multilingual}`."
    ))
}

fn cpu_thread_count() -> i32 {
    // Whisper scales well up to ~8 threads for base/small models. The old
    // `min(parallelism, 4)` cap left half the CPU idle on modern 8+ core
    // machines and directly inflated dictation latency.
    std::thread::available_parallelism()
        .map(|count| count.get().clamp(1, 8) as i32)
        .unwrap_or(4)
}

pub fn models_dir() -> Result<PathBuf, String> {
    crate::model_storage_roots()?
        .into_iter()
        .next()
        .ok_or_else(|| "Could not determine local model directory".to_string())
}

fn model_spec(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|spec| spec.id == id)
}

fn find_valid_model_path_in_roots(spec: &ModelSpec, roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .map(|root| root.join(spec.filename))
        .find(|path| validate_model_integrity(path, spec, None).is_ok())
}

fn find_valid_model_path(spec: &ModelSpec) -> Result<Option<PathBuf>, String> {
    let roots = crate::model_storage_roots()?;
    Ok(find_valid_model_path_in_roots(spec, &roots))
}

/// Fast readiness/discovery check used on the resident startup path.
///
/// Full SHA-256 verification remains mandatory before a downloaded model is
/// installed and before a model is loaded into Whisper. Startup/status calls
/// only need to know whether a plausible app-owned model is present; hashing a
/// multi-gigabyte file synchronously here would delay shortcut registration at
/// login and recreate the cold-start stall this runtime is designed to avoid.
fn find_usable_model_path_in_roots(spec: &ModelSpec, roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .map(|root| root.join(spec.filename))
        .find(|path| validate_model_file(path, None).is_ok())
}

fn find_usable_model_path(spec: &ModelSpec) -> Result<Option<PathBuf>, String> {
    let roots = crate::model_storage_roots()?;
    let legacy_roots = crate::legacy_model_storage_roots()?;
    let active_roots = roots
        .into_iter()
        .filter(|root| !legacy_roots.contains(root))
        .collect::<Vec<_>>();
    if let Some(path) = find_usable_model_path_in_roots(spec, &active_roots) {
        return Ok(Some(path));
    }
    Ok(find_valid_model_path_in_roots(spec, &legacy_roots))
}

fn model_info(spec: &ModelSpec, selected: &str) -> LocalModelInfo {
    let valid_path = find_usable_model_path(spec).ok().flatten();
    let path = valid_path.clone().unwrap_or_else(|| {
        models_dir()
            .unwrap_or_else(|_| PathBuf::from("models"))
            .join(spec.filename)
    });
    let installed_bytes = valid_path
        .as_ref()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len());
    LocalModelInfo {
        id: spec.id.to_string(),
        label: spec.label.to_string(),
        filename: spec.filename.to_string(),
        revision: spec.revision.to_string(),
        sha256: spec.sha256.to_string(),
        tier: spec.tier.to_string(),
        english_only: spec.english_only,
        recommended: spec.recommended,
        path: path.to_string_lossy().to_string(),
        downloaded: valid_path.is_some(),
        installed_bytes,
        selected: selected == spec.id,
    }
}

fn default_gpu_requested() -> bool {
    true
}

fn compiled_gpu_backend() -> Option<&'static str> {
    #[cfg(feature = "gpu-vulkan")]
    {
        return Some("Vulkan");
    }
    #[cfg(all(not(feature = "gpu-vulkan"), feature = "gpu-metal"))]
    {
        return Some("Metal");
    }
    #[allow(unreachable_code)]
    None
}

#[derive(Debug, Clone, Default)]
struct GpuLoadEvidence {
    positive: Option<String>,
    negative: Option<String>,
}

static WHISPER_LOG_INIT: Once = Once::new();
static GPU_LOAD_EVIDENCE: OnceLock<Mutex<GpuLoadEvidence>> = OnceLock::new();

fn gpu_load_evidence() -> &'static Mutex<GpuLoadEvidence> {
    GPU_LOAD_EVIDENCE.get_or_init(|| Mutex::new(GpuLoadEvidence::default()))
}

fn classify_gpu_log_line(line: &str, evidence: &mut GpuLoadEvidence) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("use gpu = 0")
        || lower.contains("whisper_backend_init_gpu: no gpu found")
        || (lower.contains("failed to initialize") && lower.contains("backend"))
    {
        evidence.negative = Some(trimmed.to_string());
        return;
    }

    // Match VoxType's runtime evidence policy: requested GPU intent is not
    // sufficient. A materialized device/backend line is positive evidence.
    // Whisper.cpp log wording varies across versions and backends
    // (Vulkan/CUDA/DML/Metal), so accept any line that couples the GPU with
    // an activation signal instead of only the exact historical strings.
    if lower.contains("found gpu device")
        || lower.contains("ggml_vulkan: found")
        || lower.contains("ggml_cuda: found")
        || lower.contains("ggml_metal: found")
        || lower.contains("ggml_dml: found")
        || (lower.contains("whisper_backend_init_gpu: using") && lower.contains("backend"))
        || (lower.contains("gpu") && lower.contains("using") && lower.contains("backend"))
        || (lower.contains("gpu") && lower.contains("backend") && lower.contains("init"))
        || (lower.contains("vulkan") && (lower.contains("device") || lower.contains("using")))
        || (lower.contains("cuda") && (lower.contains("device") || lower.contains("using")))
    {
        evidence.positive = Some(trimmed.to_string());
    }
}

#[cfg(target_os = "windows")]
type WhisperLogLevel = i32;
#[cfg(not(target_os = "windows"))]
type WhisperLogLevel = u32;

unsafe extern "C" fn whisper_log_capture(
    _level: WhisperLogLevel,
    text: *const c_char,
    _user_data: *mut c_void,
) {
    let _ = std::panic::catch_unwind(|| {
        if text.is_null() {
            return;
        }
        let line = unsafe { CStr::from_ptr(text) }.to_string_lossy();
        if let Ok(mut evidence) = gpu_load_evidence().lock() {
            classify_gpu_log_line(&line, &mut evidence);
        }
        log::debug!("whisper.cpp: {}", line.trim_end());
    });
}

fn install_whisper_log_capture() {
    WHISPER_LOG_INIT.call_once(|| unsafe {
        whisper_rs::set_log_callback(Some(whisper_log_capture), std::ptr::null_mut());
    });
}

fn reset_gpu_load_evidence() {
    if let Ok(mut evidence) = gpu_load_evidence().lock() {
        *evidence = GpuLoadEvidence::default();
    }
}

fn observed_gpu_load_evidence() -> GpuLoadEvidence {
    gpu_load_evidence()
        .lock()
        .map(|evidence| evidence.clone())
        .unwrap_or_default()
}

fn select_backend(gpu_requested: bool) -> BackendSelection {
    if !gpu_requested {
        return BackendSelection {
            active: TranscriptionComputeBackend::Cpu,
            accelerator: None,
            fallback_reason: None,
        };
    }

    let Some(compiled) = compiled_gpu_backend() else {
        return BackendSelection {
            active: TranscriptionComputeBackend::Cpu,
            accelerator: None,
            fallback_reason: Some(
                "GPU is enabled, but this build does not include a GPU transcription backend. Using CPU."
                    .to_string(),
            ),
        };
    };

    BackendSelection {
        active: TranscriptionComputeBackend::Gpu,
        accelerator: Some(compiled.to_string()),
        fallback_reason: None,
    }
}

fn resolve_loaded_backend(
    gpu_requested: bool,
    mut planned: BackendSelection,
    evidence: GpuLoadEvidence,
) -> BackendSelection {
    if !gpu_requested || planned.active == TranscriptionComputeBackend::Cpu {
        return planned;
    }
    if let Some(negative) = evidence.negative {
        return BackendSelection {
            active: TranscriptionComputeBackend::Cpu,
            accelerator: None,
            fallback_reason: Some(format!(
                "GPU was requested but Whisper could not initialize it ({negative}). Using CPU."
            )),
        };
    }
    if let Some(positive) = evidence.positive {
        if let Some(device_name) = gpu_device_name_from_log(&positive) {
            let backend_name = planned.accelerator.as_deref().unwrap_or("GPU").to_string();
            planned.accelerator = Some(format!("{backend_name} · {device_name}"));
        }
        return planned;
    }
    BackendSelection {
        active: TranscriptionComputeBackend::Cpu,
        accelerator: None,
        fallback_reason: Some(
            "GPU was requested, but Whisper did not confirm an active GPU backend during model load. Using CPU."
                .to_string(),
        ),
    }
}

fn gpu_device_name_from_log(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let marker = "found gpu device";
    let start = lower.find(marker)? + marker.len();
    let suffix = line.get(start..)?.trim_start();
    let (_, name) = suffix.split_once(':')?;
    let name = name
        .split("(type")
        .next()
        .unwrap_or(name)
        .trim()
        .trim_end_matches(',')
        .trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn status_for_model(
    model: &str,
    last_error: Option<String>,
    gpu_requested: bool,
    loaded_backend: Option<&BackendSelection>,
) -> TranscriptionRuntimeStatus {
    let (path, downloaded) = model_spec(model)
        .map(|spec| match find_usable_model_path(spec).ok().flatten() {
            Some(path) => (Some(path), true),
            None => (models_dir().ok().map(|dir| dir.join(spec.filename)), false),
        })
        .unwrap_or((None, false));
    let planned_backend = select_backend(gpu_requested);
    let active_backend = loaded_backend.map(|backend| backend.active);
    let accelerator = loaded_backend
        .and_then(|backend| backend.accelerator.clone())
        .or_else(|| planned_backend.accelerator.clone());
    let backend_fallback_reason = loaded_backend
        .and_then(|backend| backend.fallback_reason.clone())
        .or(planned_backend.fallback_reason);
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
        download_bytes: None,
        download_total_bytes: None,
        gpu_requested,
        // Never call an available device "active" until a WhisperContext was
        // successfully loaded using that execution policy.
        active_backend,
        accelerator,
        backend_fallback_reason,
        last_error,
    }
}

fn model_url(spec: &ModelSpec) -> String {
    format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/{}/{}",
        spec.revision, spec.filename
    )
}

// Structural checks are intentionally separate from the catalog trust check.
// Every supported artifact is pinned above to an immutable revision and a
// verified SHA-256; both discovery and download installation require that hash.
fn validate_model_file(path: &Path, expected_len: Option<u64>) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not stat model: {error}"))?;
    #[cfg(windows)]
    let is_reparse_point = {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    };
    #[cfg(not(windows))]
    let is_reparse_point = false;
    if metadata.file_type().is_symlink() || is_reparse_point {
        return Err("model path must not be a symbolic link or reparse point".to_string());
    }
    if !metadata.is_file() {
        return Err("model path is not a regular file".to_string());
    }
    let len = metadata.len();
    if len < MIN_MODEL_FILE_BYTES {
        return Err(format!(
            "model file is implausibly small: {len} bytes (minimum {MIN_MODEL_FILE_BYTES})"
        ));
    }
    if len > MAX_MODEL_FILE_BYTES {
        return Err(format!(
            "model file is implausibly large: {len} bytes (maximum {MAX_MODEL_FILE_BYTES})"
        ));
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

fn sha256_file(path: &Path) -> Result<String, String> {
    #[cfg(test)]
    if let Ok(mut counts) = SHA256_FILE_CALLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        *counts.entry(path.to_path_buf()).or_default() += 1;
    }
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("could not open model for hashing: {error}"))?;
    let mut hasher = Sha256::new();
    // Keep the hashing buffer off the thread stack. Release LTO can inline this
    // verifier into startup/model-discovery paths; a 1 MiB stack array is large
    // enough to exhaust the default Windows main-thread stack before the CLI
    // reaches its first prompt.
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("could not hash model file: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
fn sha256_file_call_count(path: &Path) -> usize {
    SHA256_FILE_CALLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|counts| counts.get(path).copied())
        .unwrap_or(0)
}

#[cfg(test)]
fn reset_sha256_file_call_count(path: &Path) {
    if let Ok(mut counts) = SHA256_FILE_CALLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        counts.remove(path);
    }
}

fn verified_model_identity(path: &Path, spec: &ModelSpec) -> Result<VerifiedModelIdentity, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not stat model for verification cache: {error}"))?;
    Ok(VerifiedModelIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        expected_sha256: spec.sha256,
    })
}

fn verification_cache() -> &'static Mutex<HashMap<PathBuf, VerifiedModelIdentity>> {
    VERIFIED_MODELS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn verification_cache_matches(path: &Path, identity: &VerifiedModelIdentity) -> bool {
    verification_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(path).cloned())
        .is_some_and(|cached| cached == *identity)
}

fn evict_verified_model(path: &Path) {
    if let Ok(mut cache) = verification_cache().lock() {
        cache.remove(path);
    }
}

fn validate_model_integrity(
    path: &Path,
    spec: &ModelSpec,
    expected_len: Option<u64>,
) -> Result<(), String> {
    validate_model_file(path, expected_len)?;
    let before = verified_model_identity(path, spec)?;
    if verification_cache_matches(path, &before) {
        return Ok(());
    }

    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(spec.sha256) {
        return Err(format!(
            "sha256 mismatch for '{}': expected {}, got {}",
            spec.id, spec.sha256, actual
        ));
    }

    let after = verified_model_identity(path, spec)?;
    if before != after {
        return Err(format!(
            "model '{}' changed while its integrity was being verified",
            spec.id
        ));
    }
    if let Ok(mut cache) = verification_cache().lock() {
        cache.insert(path.to_path_buf(), after);
    }
    Ok(())
}

fn remove_model_leaf_no_follow(path: &Path) -> Result<bool, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "could not inspect model path {}: {error}",
                path.display()
            ))
        }
    };

    #[cfg(windows)]
    let is_reparse_point = {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    };
    #[cfg(not(windows))]
    let is_reparse_point = false;

    evict_verified_model(path);
    if metadata.file_type().is_symlink() || is_reparse_point {
        let result = if metadata.is_dir() {
            std::fs::remove_dir(path)
        } else {
            std::fs::remove_file(path)
        };
        return result
            .map(|_| true)
            .map_err(|error| format!("failed to remove model link {}: {error}", path.display()));
    }
    if !metadata.is_file() {
        return Err(format!(
            "refusing to remove non-file model path {}",
            path.display()
        ));
    }
    std::fs::remove_file(path)
        .map(|_| true)
        .map_err(|error| format!("failed to remove model {}: {error}", path.display()))
}

fn remove_model_from_roots(spec: &ModelSpec, roots: &[PathBuf]) -> Result<bool, String> {
    let mut removed_any = false;
    for root in roots {
        for path in [
            root.join(spec.filename),
            root.join(spec.filename).with_extension("bin.part"),
        ] {
            removed_any |= remove_model_leaf_no_follow(&path)?;
        }
    }
    Ok(removed_any)
}

fn partial_download_len(path: &Path) -> Result<u64, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(format!(
                "could not inspect partial model {}: {error}",
                path.display()
            ))
        }
    };

    #[cfg(windows)]
    let is_reparse_point = {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    };
    #[cfg(not(windows))]
    let is_reparse_point = false;

    if metadata.file_type().is_symlink() || is_reparse_point || !metadata.is_file() {
        remove_model_leaf_no_follow(path)?;
        return Ok(0);
    }
    if metadata.len() > MAX_MODEL_FILE_BYTES {
        remove_model_leaf_no_follow(path)?;
        return Ok(0);
    }

    if metadata.len() >= GGML_MAGIC.len() as u64 {
        let mut magic = [0_u8; 4];
        std::fs::File::open(path)
            .and_then(|mut file| file.read_exact(&mut magic))
            .map_err(|error| format!("could not inspect partial model header: {error}"))?;
        if magic != GGML_MAGIC {
            remove_model_leaf_no_follow(path)?;
            return Ok(0);
        }
    }
    Ok(metadata.len())
}

fn parse_content_range_total(value: &str, expected_start: u64) -> Result<Option<u64>, String> {
    let value = value
        .trim()
        .strip_prefix("bytes ")
        .ok_or_else(|| format!("invalid Content-Range: {value}"))?;
    let (range, total) = value
        .split_once('/')
        .ok_or_else(|| format!("invalid Content-Range: {value}"))?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| format!("invalid Content-Range: {value}"))?;
    let start = start
        .parse::<u64>()
        .map_err(|_| format!("invalid Content-Range start: {value}"))?;
    let end = end
        .parse::<u64>()
        .map_err(|_| format!("invalid Content-Range end: {value}"))?;
    if start != expected_start || end < start {
        return Err(format!(
            "unexpected Content-Range '{value}' for resume offset {expected_start}"
        ));
    }
    if total == "*" {
        return Ok(None);
    }
    let total = total
        .parse::<u64>()
        .map_err(|_| format!("invalid Content-Range total: {value}"))?;
    if end >= total {
        return Err(format!("invalid Content-Range total: {value}"));
    }
    validate_declared_model_size(total)?;
    Ok(Some(total))
}

fn validate_declared_model_size(size: u64) -> Result<(), String> {
    if !(MIN_MODEL_FILE_BYTES..=MAX_MODEL_FILE_BYTES).contains(&size) {
        return Err(format!(
            "Model download declared an invalid size of {size} bytes"
        ));
    }
    Ok(())
}

fn download_response_plan(
    status: StatusCode,
    requested_offset: u64,
    content_range: Option<&str>,
    content_length: Option<u64>,
) -> Result<(bool, u64, Option<u64>), String> {
    if requested_offset > 0 && status == StatusCode::PARTIAL_CONTENT {
        let content_range =
            content_range.ok_or_else(|| "Resume response was missing Content-Range".to_string())?;
        let total = parse_content_range_total(content_range, requested_offset)?;
        return Ok((true, requested_offset, total));
    }

    if status == StatusCode::PARTIAL_CONTENT {
        let content_range = content_range
            .ok_or_else(|| "Partial response was missing Content-Range".to_string())?;
        let total = parse_content_range_total(content_range, 0)?;
        return Ok((false, 0, total));
    }

    if let Some(total) = content_length {
        validate_declared_model_size(total)?;
    }
    // A 200 response to a Range request means the origin ignored Range. The
    // caller must truncate and restart from byte 0 rather than append.
    Ok((false, 0, content_length))
}

fn load_model(
    loaded: &mut Option<LoadedModel>,
    model: &str,
    path: &Path,
    gpu_requested: bool,
) -> Result<(), String> {
    install_whisper_log_capture();
    reset_gpu_load_evidence();
    let planned_backend = select_backend(gpu_requested);
    let mut context_params = WhisperContextParameters::default();
    context_params.use_gpu(matches!(
        planned_backend.active,
        TranscriptionComputeBackend::Gpu
    ));
    // Flash attention cuts attention memory traffic on both CPU and GPU
    // paths; it is a pure latency win for short dictation segments.
    context_params.flash_attn(true);
    context_params.gpu_device(0);
    let model_path = path
        .to_str()
        .ok_or_else(|| "Local model path is not valid UTF-8".to_string())?;
    let attempted_gpu = planned_backend.active == TranscriptionComputeBackend::Gpu;
    let context = match WhisperContext::new_with_params(model_path, context_params) {
        Ok(context) => context,
        Err(gpu_error) if attempted_gpu => {
            log::warn!(
                "GPU Whisper context initialization failed for '{}': {}. Retrying on CPU.",
                model,
                gpu_error
            );
            reset_gpu_load_evidence();
            let mut cpu_params = WhisperContextParameters::default();
            cpu_params.use_gpu(false);
            cpu_params.flash_attn(true);
            cpu_params.gpu_device(0);
            let context = WhisperContext::new_with_params(model_path, cpu_params).map_err(|error| {
                format!(
                    "Failed to load local Whisper model on GPU ({gpu_error}) and CPU fallback ({error})"
                )
            })?;
            let backend = BackendSelection {
                active: TranscriptionComputeBackend::Cpu,
                accelerator: None,
                fallback_reason: Some(format!(
                    "GPU initialization failed ({gpu_error}). Using CPU."
                )),
            };
            *loaded = Some(LoadedModel {
                id: model.to_string(),
                gpu_requested,
                backend,
                context,
            });
            return Ok(());
        }
        Err(error) => {
            return Err(format!("Failed to load local Whisper model: {error}"));
        }
    };
    let backend =
        resolve_loaded_backend(gpu_requested, planned_backend, observed_gpu_load_evidence());
    log::info!(
        "Local transcription backend loaded: model={}, gpu_requested={}, active={:?}, accelerator={}, fallback={}",
        model,
        gpu_requested,
        backend.active,
        backend.accelerator.as_deref().unwrap_or("none"),
        backend.fallback_reason.as_deref().unwrap_or("none")
    );
    #[cfg(debug_assertions)]
    eprintln!(
        "[Origin Speak backend] model={} gpu_requested={} active={:?} accelerator={} fallback={}",
        model,
        gpu_requested,
        backend.active,
        backend.accelerator.as_deref().unwrap_or("none"),
        backend.fallback_reason.as_deref().unwrap_or("none")
    );
    *loaded = Some(LoadedModel {
        id: model.to_string(),
        gpu_requested,
        backend,
        context,
    });
    Ok(())
}

fn run_whisper_inference(
    context: &WhisperContext,
    audio: &[f32],
    model: &str,
    language: Option<&str>,
    dictionary_hints: &[String],
) -> Result<String, WhisperInferenceError> {
    let mut state = context
        .create_state()
        .map_err(|error| WhisperInferenceError::State(error.to_string()))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(cpu_thread_count());
    params.set_translate(false);
    params.set_detect_language(false);
    // Dictation only needs the text: skipping timestamp prediction removes a
    // meaningful chunk of per-segment decoder work.
    params.set_no_timestamps(true);
    params.set_token_timestamps(false);
    params.set_temperature(0.0);
    params.set_split_on_word(false);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    params.set_no_context(true);
    params.set_single_segment(audio.len() < WHISPER_SAMPLE_RATE as usize * 30);

    let selected_language = if model.ends_with(".en") {
        Some("en")
    } else {
        language.filter(|value| !value.trim().is_empty() && !value.eq_ignore_ascii_case("auto"))
    };
    params.set_language(selected_language);

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
        .full(params, audio)
        .map_err(|error| WhisperInferenceError::Full(error.to_string()))?;

    let mut text = String::new();
    for segment in state.as_iter() {
        text.push_str(
            segment
                .to_str()
                .map_err(|error| WhisperInferenceError::Output(error.to_string()))?,
        );
    }
    Ok(text.trim().to_string())
}

fn should_retry_inference_on_cpu(
    backend: &BackendSelection,
    error: &WhisperInferenceError,
) -> bool {
    backend.active == TranscriptionComputeBackend::Gpu && error.full_error().is_some()
}

fn cpu_backend_after_gpu_inference_failure(gpu_error: &str) -> BackendSelection {
    BackendSelection {
        active: TranscriptionComputeBackend::Cpu,
        accelerator: None,
        fallback_reason: Some(format!(
            "GPU inference failed ({gpu_error}). Retried on CPU."
        )),
    }
}

fn load_cpu_after_gpu_inference_failure(
    loaded: &mut Option<LoadedModel>,
    model: &str,
    path: &Path,
    gpu_requested: bool,
    gpu_error: &str,
) -> Result<(), String> {
    let model_path = path
        .to_str()
        .ok_or_else(|| "Local model path is not valid UTF-8".to_string())?;
    let mut cpu_params = WhisperContextParameters::default();
    cpu_params.use_gpu(false);
    cpu_params.flash_attn(true);
    cpu_params.gpu_device(0);
    let context = WhisperContext::new_with_params(model_path, cpu_params).map_err(|cpu_error| {
        format!(
            "GPU inference failed ({gpu_error}); failed to load CPU fallback model ({cpu_error})"
        )
    })?;
    *loaded = Some(LoadedModel {
        id: model.to_string(),
        gpu_requested,
        backend: cpu_backend_after_gpu_inference_failure(gpu_error),
        context,
    });
    Ok(())
}

fn ensure_model_loaded(
    loaded: &Arc<Mutex<Option<LoadedModel>>>,
    model: &str,
    path: &Path,
    gpu_requested: bool,
) -> Result<(), String> {
    let mut guard = loaded
        .lock()
        .map_err(|_| "Local transcription model lock was poisoned".to_string())?;
    if guard
        .as_ref()
        .is_some_and(|loaded| loaded.id == model && loaded.gpu_requested == gpu_requested)
    {
        return Ok(());
    }
    load_model(&mut guard, model, path, gpu_requested)
}

async fn download_model_file<F>(
    spec: &ModelSpec,
    destination: &Path,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u64, Option<u64>),
{
    let url = model_url(spec);
    let part = destination.with_extension("bin.part");
    if validate_model_integrity(&part, spec, None).is_ok() {
        remove_model_leaf_no_follow(destination)?;
        return tokio::fs::rename(&part, destination)
            .await
            .map_err(|error| format!("Failed to install completed local model: {error}"));
    }

    let client = reqwest::Client::new();
    let mut resume_offset = partial_download_len(&part)?;
    let response = loop {
        let mut request = client.get(&url);
        if resume_offset > 0 {
            request = request.header(RANGE, format!("bytes={resume_offset}-"));
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("Failed to download local model '{}': {error}", spec.id))?;

        if resume_offset > 0 && response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
            // A stale/oversized partial cannot be resumed against this immutable
            // artifact. Remove only the partial leaf and retry once from byte 0.
            remove_model_leaf_no_follow(&part)?;
            resume_offset = 0;
            continue;
        }
        break response;
    };

    if !response.status().is_success() {
        return Err(format!(
            "Model download failed with HTTP {}",
            response.status()
        ));
    }

    let content_range = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok());
    let (append, start_offset, expected_total) = download_response_plan(
        response.status(),
        resume_offset,
        content_range,
        response.content_length(),
    )?;

    on_progress(start_offset, expected_total);
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true);
    if append {
        options.append(true);
    } else {
        options.create(true).truncate(true);
    }
    let mut file = options
        .open(&part)
        .await
        .map_err(|error| format!("Failed to open model download file: {error}"))?;
    let mut stream = response.bytes_stream();
    let mut downloaded = start_offset;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("Model download interrupted: {error}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("Failed to write model download: {error}"))?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > MAX_MODEL_FILE_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            return Err(format!(
                "Model download exceeded the {MAX_MODEL_FILE_BYTES}-byte safety limit"
            ));
        }
        on_progress(downloaded, expected_total);
    }
    file.flush()
        .await
        .map_err(|error| format!("Failed to flush model download: {error}"))?;
    file.sync_all()
        .await
        .map_err(|error| format!("Failed to sync model download: {error}"))?;
    drop(file);

    if let Some(expected_total) = expected_total {
        if downloaded != expected_total {
            return Err(format!(
                "Model download interrupted: received {downloaded} of {expected_total} bytes; partial file retained for resume"
            ));
        }
    }

    if downloaded < MIN_MODEL_FILE_BYTES {
        return Err(format!(
            "Model download incomplete: only {downloaded} bytes received; partial file retained for resume"
        ));
    }

    if let Err(error) = validate_model_integrity(&part, spec, expected_total) {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(format!(
            "Downloaded model '{}' is invalid: {error}",
            spec.id
        ));
    }

    remove_model_leaf_no_follow(destination)?;
    tokio::fs::rename(&part, destination)
        .await
        .map_err(|error| format!("Failed to install downloaded local model: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("origin-speak-{name}-{}", uuid::Uuid::new_v4()))
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
        let mut file = std::fs::File::create(&valid).unwrap();
        use std::io::{Seek, SeekFrom, Write};
        file.write_all(&GGML_MAGIC).unwrap();
        file.seek(SeekFrom::Start(MIN_MODEL_FILE_BYTES - 1))
            .unwrap();
        file.write_all(&[0]).unwrap();
        drop(file);
        validate_model_file(&valid, Some(MIN_MODEL_FILE_BYTES)).expect("valid model header");
        assert!(validate_model_file(&valid, Some(MIN_MODEL_FILE_BYTES + 1)).is_err());

        let html = temp_file("html-model.bin");
        std::fs::write(&html, b"<html>error</html>").unwrap();
        assert!(validate_model_file(&html, None).is_err());

        let _ = std::fs::remove_file(valid);
        let _ = std::fs::remove_file(html);
    }

    #[test]
    fn catalog_exposes_dictation_selection_metadata() {
        let base = model_spec("base.en").unwrap();
        assert_eq!(base.tier, "base");
        assert!(base.english_only);
        assert!(base.recommended);
        assert!(!model_spec("base").unwrap().english_only);
    }

    #[test]
    fn catalog_pins_verified_revisions_and_sha256_digests() {
        let expected = [
            (
                "tiny",
                "80da2d8",
                "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
            ),
            (
                "tiny.en",
                "80da2d8",
                "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f",
            ),
            (
                "base",
                "80da2d8",
                "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
            ),
            (
                "base.en",
                "80da2d8",
                "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
            ),
            (
                "small",
                "80da2d8",
                "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
            ),
            (
                "small.en",
                "80da2d8",
                "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
            ),
            (
                "medium",
                "80da2d8",
                "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
            ),
            (
                "medium.en",
                "80da2d8",
                "cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356",
            ),
            (
                "large-v3",
                "362722b",
                "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
            ),
            (
                "large-v3-turbo",
                "98aa99a",
                "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
            ),
        ];
        for (id, revision, sha256) in expected {
            let spec = model_spec(id).expect("catalog model");
            assert_eq!(spec.revision, revision, "revision mismatch for {id}");
            assert_eq!(spec.sha256, sha256, "sha256 mismatch for {id}");
        }
    }

    #[test]
    fn legacy_model_discovery_prefers_current_then_legacy_without_parent_deletion() {
        let root = temp_file("model-roots");
        let current = root.join("current");
        let legacy = root.join("legacy");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        let catalog_spec = model_spec("tiny.en").unwrap();
        let legacy_model = legacy.join(catalog_spec.filename);
        let mut file = std::fs::File::create(&legacy_model).unwrap();
        use std::io::{Seek, SeekFrom, Write};
        file.write_all(&GGML_MAGIC).unwrap();
        file.seek(SeekFrom::Start(MIN_MODEL_FILE_BYTES - 1))
            .unwrap();
        file.write_all(&[0]).unwrap();
        drop(file);

        let sha256: &'static str = Box::leak(sha256_file(&legacy_model).unwrap().into_boxed_str());
        let spec = ModelSpec {
            sha256,
            ..*catalog_spec
        };

        assert_eq!(
            find_valid_model_path_in_roots(&spec, &[current.clone(), legacy.clone()]),
            Some(legacy_model.clone())
        );

        std::fs::remove_file(legacy_model).unwrap();
        std::fs::remove_dir(legacy).unwrap();
        std::fs::remove_dir(current).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn model_removal_cleans_current_and_legacy_roots_without_deleting_parents() {
        let root = temp_file("model-remove");
        let current = root.join("current");
        let legacy = root.join("legacy");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        let spec = model_spec("base.en").unwrap();
        let current_model = current.join(spec.filename);
        let legacy_model = legacy.join(spec.filename);
        let current_part = current_model.with_extension("bin.part");
        let legacy_part = legacy_model.with_extension("bin.part");
        let current_sibling = current.join("keep.txt");
        let legacy_sibling = legacy.join("keep.txt");
        for path in [&current_model, &legacy_model, &current_part, &legacy_part] {
            std::fs::write(path, b"temporary").unwrap();
        }
        std::fs::write(&current_sibling, b"keep").unwrap();
        std::fs::write(&legacy_sibling, b"keep").unwrap();

        assert!(remove_model_from_roots(spec, &[current.clone(), legacy.clone()]).unwrap());
        for path in [&current_model, &legacy_model, &current_part, &legacy_part] {
            assert!(!path.exists());
        }
        assert!(current_sibling.exists());
        assert!(legacy_sibling.exists());
        assert!(current.exists());
        assert!(legacy.exists());

        std::fs::remove_file(current_sibling).unwrap();
        std::fs::remove_file(legacy_sibling).unwrap();
        std::fs::remove_dir(current).unwrap();
        std::fs::remove_dir(legacy).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn model_urls_point_to_whisper_cpp_artifacts() {
        let spec = model_spec("base.en").unwrap();
        let url = model_url(spec);
        assert!(url.ends_with("/ggml-base.en.bin"));
        assert!(url.contains("huggingface.co/ggerganov/whisper.cpp"));
        assert!(url.contains("/resolve/80da2d8/"));
    }

    #[test]
    fn sha256_helper_matches_known_vector_and_integrity_rejects_wrong_digest() {
        let small = temp_file("sha256-vector");
        std::fs::write(&small, b"abc").unwrap();
        assert_eq!(
            sha256_file(&small).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_file(small).unwrap();

        let model = temp_file("wrong-digest.bin");
        let mut file = std::fs::File::create(&model).unwrap();
        use std::io::{Seek, SeekFrom, Write};
        file.write_all(&GGML_MAGIC).unwrap();
        file.seek(SeekFrom::Start(MIN_MODEL_FILE_BYTES - 1))
            .unwrap();
        file.write_all(&[0]).unwrap();
        drop(file);
        let error = validate_model_integrity(&model, model_spec("base.en").unwrap(), None)
            .expect_err("wrong digest must not be trusted");
        assert!(error.contains("sha256 mismatch"));
        std::fs::remove_file(model).unwrap();
    }

    #[test]
    fn sha256_helper_fits_on_a_small_thread_stack() {
        let small = temp_file("sha256-small-stack");
        std::fs::write(&small, b"abc").unwrap();
        let thread_path = small.clone();
        let digest = std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || sha256_file(&thread_path))
            .expect("spawn small-stack hashing thread")
            .join()
            .expect("small-stack hashing thread panicked")
            .expect("hash small-stack vector");
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_file(small).unwrap();
    }

    #[test]
    fn verified_model_cache_avoids_rehash_and_invalidates_on_identity_changes() {
        let model = temp_file("verified-cache.bin");
        let mut file = std::fs::File::create(&model).unwrap();
        use std::io::{Seek, SeekFrom, Write};
        file.write_all(&GGML_MAGIC).unwrap();
        file.seek(SeekFrom::Start(MIN_MODEL_FILE_BYTES - 1))
            .unwrap();
        file.write_all(&[0]).unwrap();
        drop(file);

        let digest: &'static str = Box::leak(sha256_file(&model).unwrap().into_boxed_str());
        let spec = ModelSpec {
            sha256: digest,
            ..*model_spec("base.en").unwrap()
        };
        evict_verified_model(&model);
        reset_sha256_file_call_count(&model);

        validate_model_integrity(&model, &spec, None).expect("first verification");
        assert_eq!(sha256_file_call_count(&model), 1);
        validate_model_integrity(&model, &spec, None).expect("cached verification");
        assert_eq!(
            sha256_file_call_count(&model),
            1,
            "unchanged metadata and spec must not rehash"
        );

        let base_identity = verified_model_identity(&model, &spec).unwrap();
        let different_size = VerifiedModelIdentity {
            len: base_identity.len + 1,
            ..base_identity.clone()
        };
        let different_mtime = VerifiedModelIdentity {
            modified: Some(SystemTime::UNIX_EPOCH),
            ..base_identity.clone()
        };
        let different_spec = VerifiedModelIdentity {
            expected_sha256: model_spec("small.en").unwrap().sha256,
            ..base_identity.clone()
        };
        assert!(verification_cache_matches(&model, &base_identity));
        assert!(!verification_cache_matches(&model, &different_size));
        assert!(!verification_cache_matches(&model, &different_mtime));
        assert!(!verification_cache_matches(&model, &different_spec));

        evict_verified_model(&model);
        std::fs::remove_file(model).unwrap();
    }

    #[test]
    fn startup_model_discovery_never_hashes_the_model_file() {
        let root = temp_file("startup-discovery-root");
        let _ = std::fs::remove_file(&root);
        std::fs::create_dir_all(&root).unwrap();
        let spec = *model_spec("base.en").unwrap();
        let model = root.join(spec.filename);
        let mut file = std::fs::File::create(&model).unwrap();
        use std::io::{Seek, SeekFrom, Write};
        file.write_all(&GGML_MAGIC).unwrap();
        file.seek(SeekFrom::Start(MIN_MODEL_FILE_BYTES - 1))
            .unwrap();
        file.write_all(&[0]).unwrap();
        drop(file);

        reset_sha256_file_call_count(&model);
        assert_eq!(
            find_usable_model_path_in_roots(&spec, std::slice::from_ref(&root)),
            Some(model.clone())
        );
        assert_eq!(
            sha256_file_call_count(&model),
            0,
            "resident startup/status discovery must stay metadata/header-only"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resume_plan_appends_only_for_matching_partial_content() {
        let offset = MIN_MODEL_FILE_BYTES;
        let total = MIN_MODEL_FILE_BYTES * 3;
        let range = format!("bytes {offset}-{}/{total}", total - 1);
        let plan = download_response_plan(
            StatusCode::PARTIAL_CONTENT,
            offset,
            Some(&range),
            Some(total - offset),
        )
        .unwrap();
        assert_eq!(plan, (true, offset, Some(total)));

        let restarted = download_response_plan(StatusCode::OK, offset, None, Some(total)).unwrap();
        assert_eq!(restarted, (false, 0, Some(total)));
        assert!(download_response_plan(
            StatusCode::PARTIAL_CONTENT,
            offset,
            Some("bytes 0-1048575/3145728"),
            Some(MIN_MODEL_FILE_BYTES),
        )
        .is_err());
    }

    #[test]
    fn valid_partial_file_is_retained_for_resume() {
        let part = temp_file("resume.bin.part");
        std::fs::write(&part, [b'l', b'm', b'g', b'g', 1, 2, 3, 4]).unwrap();
        assert_eq!(partial_download_len(&part).unwrap(), 8);
        assert!(part.exists());
        std::fs::remove_file(part).unwrap();
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
        assert!(error.contains("origin model download base"));
        assert!(error.contains("origin model use base"));
    }

    #[test]
    fn download_status_names_the_downloaded_model_without_overwriting_transcription() {
        let selected = "base.en";
        let service = TranscriptionService {
            settings: Arc::new(Mutex::new(TranscriptionSettings {
                model: selected.to_string(),
            })),
            runtime: Arc::new(Mutex::new(status_for_model(selected, None, true, None))),
            download_runtime: Arc::new(Mutex::new(Some(TranscriptionRuntimeStatus {
                phase: TranscriptionRuntimePhase::Downloading,
                model: "small.en".to_string(),
                model_path: "small.en.part".to_string(),
                model_downloaded: false,
                download_bytes: Some(1_024),
                download_total_bytes: Some(4_096),
                gpu_requested: true,
                active_backend: None,
                accelerator: None,
                backend_fallback_reason: None,
                last_error: None,
            }))),
            download_guard: Arc::new(tokio::sync::Mutex::new(())),
            loaded: Arc::new(Mutex::new(None)),
            gpu_enabled: Arc::new(AtomicBool::new(true)),
        };

        let downloading = service.runtime_status();
        assert_eq!(downloading.phase, TranscriptionRuntimePhase::Downloading);
        assert_eq!(downloading.model, "small.en");
        assert!(!downloading.model_downloaded);
        assert_eq!(downloading.download_bytes, Some(1_024));
        assert_eq!(downloading.download_total_bytes, Some(4_096));

        service.set_runtime(TranscriptionRuntimeStatus {
            phase: TranscriptionRuntimePhase::Transcribing,
            model: selected.to_string(),
            model_path: "base.en".to_string(),
            model_downloaded: true,
            download_bytes: None,
            download_total_bytes: None,
            gpu_requested: true,
            active_backend: Some(TranscriptionComputeBackend::Cpu),
            accelerator: None,
            backend_fallback_reason: Some("test fallback".to_string()),
            last_error: None,
        });
        let transcribing = service.runtime_status();
        assert_eq!(transcribing.phase, TranscriptionRuntimePhase::Transcribing);
        assert_eq!(transcribing.model, selected);
    }

    #[test]
    fn gpu_status_requires_positive_engine_evidence() {
        let planned = BackendSelection {
            active: TranscriptionComputeBackend::Gpu,
            accelerator: Some("Vulkan".to_string()),
            fallback_reason: None,
        };
        let unresolved = resolve_loaded_backend(true, planned.clone(), GpuLoadEvidence::default());
        assert_eq!(unresolved.active, TranscriptionComputeBackend::Cpu);
        assert!(unresolved.fallback_reason.is_some());

        let positive = resolve_loaded_backend(
            true,
            planned.clone(),
            GpuLoadEvidence {
                positive: Some("whisper_backend_init_gpu: using Vulkan0 backend".to_string()),
                negative: None,
            },
        );
        assert_eq!(positive.active, TranscriptionComputeBackend::Gpu);

        let failed = resolve_loaded_backend(
            true,
            planned,
            GpuLoadEvidence {
                positive: Some("whisper_backend_init_gpu: found GPU device 0".to_string()),
                negative: Some(
                    "whisper_backend_init_gpu: failed to initialize Vulkan0 backend".to_string(),
                ),
            },
        );
        assert_eq!(failed.active, TranscriptionComputeBackend::Cpu);
        assert!(failed
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("failed to initialize")));
    }

    #[test]
    fn gpu_log_classifier_does_not_treat_intent_as_activation() {
        let mut evidence = GpuLoadEvidence::default();
        classify_gpu_log_line("whisper_init_state: use gpu = 1", &mut evidence);
        assert!(evidence.positive.is_none());
        assert!(evidence.negative.is_none());

        classify_gpu_log_line(
            "whisper_backend_init_gpu: found GPU device 0: NVIDIA GeForce RTX 4080",
            &mut evidence,
        );
        assert!(evidence.positive.is_some());

        classify_gpu_log_line(
            "whisper_backend_init_gpu: failed to initialize Vulkan0 backend",
            &mut evidence,
        );
        assert!(evidence.negative.is_some());

        assert_eq!(
            gpu_device_name_from_log(
                "whisper_backend_init_gpu: found GPU device 0: NVIDIA GeForce RTX 4080 (type: 1)"
            )
            .as_deref(),
            Some("NVIDIA GeForce RTX 4080")
        );
    }

    #[test]
    fn gpu_full_failure_is_the_only_inference_error_that_retries_on_cpu() {
        let gpu = BackendSelection {
            active: TranscriptionComputeBackend::Gpu,
            accelerator: Some("Vulkan".to_string()),
            fallback_reason: None,
        };
        let cpu = BackendSelection {
            active: TranscriptionComputeBackend::Cpu,
            accelerator: None,
            fallback_reason: None,
        };
        let full_error = WhisperInferenceError::Full("device lost".to_string());
        let state_error = WhisperInferenceError::State("state failed".to_string());
        let output_error = WhisperInferenceError::Output("invalid text".to_string());

        assert!(should_retry_inference_on_cpu(&gpu, &full_error));
        assert!(!should_retry_inference_on_cpu(&cpu, &full_error));
        assert!(!should_retry_inference_on_cpu(&gpu, &state_error));
        assert!(!should_retry_inference_on_cpu(&gpu, &output_error));
    }

    #[test]
    fn gpu_inference_fallback_status_is_truthfully_cpu() {
        let fallback = cpu_backend_after_gpu_inference_failure("device lost");
        assert_eq!(fallback.active, TranscriptionComputeBackend::Cpu);
        assert!(fallback.accelerator.is_none());
        assert!(fallback
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("device lost") && reason.contains("CPU")));
    }
}
