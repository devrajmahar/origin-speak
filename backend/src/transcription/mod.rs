mod resampler;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::ffi::{c_char, c_void, CStr};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Once, OnceLock,
};
use std::time::Instant;
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
        let dir = models_dir()?;
        Ok(MODELS
            .iter()
            .map(|spec| model_info(spec, &dir, &selected))
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

        if validate_model_file(&destination, None).is_ok() {
            self.set_download_runtime(None);
            return Ok(model_info(&spec, &dir, &selected));
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
                Ok(model_info(&spec, &dir, &selected))
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

    /// Load the selected local Whisper model into memory without running inference.
    /// This is safe to call repeatedly and is intentionally run off the UI thread so
    /// the first short dictation does not pay the full model-open cost after release.
    pub async fn preload_selected_model(&self) -> Result<(), String> {
        let model = self.settings().model;
        let spec = *model_spec(&model)
            .ok_or_else(|| format!("Unknown local transcription model: {model}"))?;
        let path = models_dir()?.join(spec.filename);
        validate_model_file(&path, None).map_err(|error| {
            format!("Local model '{model}' is not installed or is invalid: {error}")
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
            "[ListenOS timing] model_preload model={} elapsed_ms={}",
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
        let path = models_dir()?.join(spec.filename);
        validate_model_file(&path, None).map_err(|error| {
            format!("Local model '{model}' is not installed or is invalid: {error}. Download it before dictating.")
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
                "[ListenOS timing] transcription model={} audio_ms={} resample_ms={} model_load_ms={} inference_ms={} total_ms={} text_chars={}",
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
        "Local model '{model}' is English-only and cannot transcribe source language '{language}'. Select or download the multilingual '{multilingual}' model in Settings -> System."
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

unsafe extern "C" fn whisper_log_capture(
    _level: i32,
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
    let path =
        model_spec(model).and_then(|spec| models_dir().ok().map(|dir| dir.join(spec.filename)));
    let downloaded = path
        .as_ref()
        .map(|path| validate_model_file(path, None).is_ok())
        .unwrap_or(false);
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
        "[ListenOS backend] model={} gpu_requested={} active={:?} accelerator={} fallback={}",
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
    on_progress(0, expected_len);
    let part = destination.with_extension("bin.part");
    let _ = tokio::fs::remove_file(&part).await;
    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|error| format!("Failed to create model download file: {error}"))?;
    let mut stream = response.bytes_stream();
    let mut downloaded = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("Model download interrupted: {error}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("Failed to write model download: {error}"))?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        on_progress(downloaded, expected_len);
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
