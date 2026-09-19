//! Origin Speak dictation engine.

mod audio;
mod commands;
mod config;
mod delivery;
mod dictionary;
mod error_log;
mod paths;
mod shortcuts;
mod streaming;
mod transcription;

use std::{ops::Deref, sync::Arc};
use tokio::sync::Mutex;

use audio::AudioState;
use delivery::DeliveryState;
use error_log::ErrorLog;
use streaming::AudioStreamer;

pub use audio::AudioDevice;
pub use commands::*;
pub use config::{AppConfig, LanguagePreferences};
pub use delivery::{DeliveryPhase, DeliveryStatusSnapshot, TargetSurfaceKind};
pub use dictionary::DictionaryWord;
pub use paths::{
    app_data_root, app_local_data_root, legacy_app_data_root, legacy_app_local_data_root,
    legacy_model_storage_roots, model_storage_roots,
};
pub use shortcuts::{ShortcutEvent, ShortcutService};
pub use transcription::{
    LocalModelInfo, TranscriptionComputeBackend, TranscriptionRuntimePhase,
    TranscriptionRuntimeStatus, TranscriptionService, TranscriptionSettings,
};

/// Keep native transcribe.cpp device-registration logs out of user-facing CLI
/// output. The resident runtime does not call this and retains its normal
/// diagnostics.
pub fn disable_cli_transcription_logging() {
    transcribe_cpp::disable_logging();
}

#[derive(Clone, Copy)]
pub struct State<'a, T>(&'a T);

impl<'a, T> State<'a, T> {
    pub fn new(inner: &'a T) -> Self {
        Self(inner)
    }
}

impl<T> Deref for State<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

pub struct AppState {
    pub(crate) audio: Arc<Mutex<AudioState>>,
    pub config: Arc<Mutex<AppConfig>>,
    pub(crate) config_update_lock: Arc<Mutex<()>>,
    pub(crate) streamer: Arc<Mutex<AudioStreamer>>,
    pub transcription: Arc<TranscriptionService>,
    pub(crate) is_listening: Arc<Mutex<bool>>,
    pub(crate) is_processing: Arc<Mutex<bool>>,
    pub(crate) delivery: Arc<std::sync::Mutex<DeliveryState>>,
    pub(crate) error_log: Arc<Mutex<ErrorLog>>,
}

impl Default for AppState {
    fn default() -> Self {
        let app_config = AppConfig::load_from_disk().unwrap_or_default();
        let audio_state = AudioState {
            selected_device: app_config.selected_audio_device.clone(),
            ..AudioState::default()
        };
        let transcription = Arc::new(TranscriptionService::new_with_gpu_enabled(
            app_config.use_gpu,
        ));

        Self {
            audio: Arc::new(Mutex::new(audio_state)),
            config: Arc::new(Mutex::new(app_config)),
            config_update_lock: Arc::new(Mutex::new(())),
            streamer: Arc::new(Mutex::new(AudioStreamer::new())),
            transcription,
            is_listening: Arc::new(Mutex::new(false)),
            is_processing: Arc::new(Mutex::new(false)),
            delivery: Arc::new(std::sync::Mutex::new(DeliveryState::new())),
            error_log: Arc::new(Mutex::new(ErrorLog::new())),
        }
    }
}
