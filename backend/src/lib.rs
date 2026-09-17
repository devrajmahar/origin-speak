//! ListenOS native voice engine.
//!
//! Electron owns windows, tray, deep links, autostart, and updates. This crate
//! stays focused on audio, AI processing, persistence, and system automation.

mod audio;
mod clipboard;
mod commands;
mod config;
mod conversation;
mod correction;
mod delivery;
mod dictionary;
mod error_log;
mod integrations;
mod ipc;
mod notes;
mod snippets;
mod streaming;
mod system;
mod transcription;
mod voice;

use std::{ops::Deref, sync::Arc};
use tokio::sync::Mutex;

pub use audio::AudioState;
pub use clipboard::ClipboardService;
pub use commands::*;
pub use config::AppConfig;
pub use conversation::{ConversationMemory, ConversationStore, Fact, Message, Role};
pub use correction::CorrectionTracker;
pub use delivery::{DeliveryPhase, DeliveryState, DeliveryStatusSnapshot, TargetSurfaceKind};
pub use dictionary::{DictionaryStore, DictionaryWord};
pub use error_log::{ErrorEntry, ErrorLog, ErrorType};
pub use integrations::{AppIntegration, IntegrationManager};
pub use notes::{Note, NotesStore};
pub use snippets::{Snippet, SnippetsStore};
pub use streaming::{AudioHealthPhase, AudioRuntimeStatus, AudioStreamer, SAMPLE_RATE};
pub use transcription::{
    LocalModelInfo, TranscriptionRuntimePhase, TranscriptionRuntimeStatus, TranscriptionService,
    TranscriptionSettings,
};
pub use voice::{VoiceContext, VoiceMode};

/// Borrowed state wrapper used by native command handlers.
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

/// Shared application state used by every JSON-RPC request from Electron.
pub struct AppState {
    pub audio: Arc<Mutex<AudioState>>,
    pub config: Arc<Mutex<AppConfig>>,
    pub streamer: Arc<Mutex<AudioStreamer>>,
    pub transcription: Arc<TranscriptionService>,
    pub is_listening: Arc<Mutex<bool>>,
    pub is_processing: Arc<Mutex<bool>>,
    pub current_context: Arc<Mutex<VoiceContext>>,
    pub history: Arc<Mutex<Vec<VoiceProcessingResult>>>,
    pub conversation: Arc<Mutex<ConversationMemory>>,
    pub conversation_store: Arc<std::sync::Mutex<Option<ConversationStore>>>,
    pub clipboard: Arc<Mutex<ClipboardService>>,
    pub integrations: Arc<Mutex<IntegrationManager>>,
    pub correction_tracker: Arc<Mutex<CorrectionTracker>>,
    pub error_log: Arc<Mutex<ErrorLog>>,
    pub pending_action: Arc<Mutex<Option<commands::PendingAction>>>,
    pub delivery: Arc<std::sync::Mutex<DeliveryState>>,
}

impl Default for AppState {
    fn default() -> Self {
        let conversation_store = ConversationStore::new().ok();
        let mut conversation = ConversationMemory::new_session();
        if let Some(ref store) = conversation_store {
            if let Ok(facts) = store.load_facts() {
                conversation.extracted_facts = facts;
            }
        }

        let mut app_config = AppConfig::default();
        if let Some(saved_languages) = config::LanguagePreferences::load_from_disk() {
            app_config.language_preferences = saved_languages;
        }
        if let Some(saved_vibe) = config::VibeCodingConfig::load_from_disk() {
            app_config.vibe_coding = saved_vibe;
        }

        Self {
            audio: Arc::new(Mutex::new(AudioState::default())),
            config: Arc::new(Mutex::new(app_config)),
            streamer: Arc::new(Mutex::new(AudioStreamer::new())),
            transcription: Arc::new(TranscriptionService::new()),
            is_listening: Arc::new(Mutex::new(false)),
            is_processing: Arc::new(Mutex::new(false)),
            current_context: Arc::new(Mutex::new(VoiceContext::default())),
            history: Arc::new(Mutex::new(Vec::new())),
            conversation: Arc::new(Mutex::new(conversation)),
            conversation_store: Arc::new(std::sync::Mutex::new(conversation_store)),
            clipboard: Arc::new(Mutex::new(ClipboardService::new())),
            integrations: Arc::new(Mutex::new(IntegrationManager::new())),
            correction_tracker: Arc::new(Mutex::new(CorrectionTracker::new())),
            error_log: Arc::new(Mutex::new(ErrorLog::new())),
            pending_action: Arc::new(Mutex::new(None)),
            delivery: Arc::new(std::sync::Mutex::new(DeliveryState::new())),
        }
    }
}

pub fn run() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let env_path = std::path::PathBuf::from(manifest_dir).join("../.env.local");
    if env_path.exists() {
        let _ = dotenvy::from_path(env_path);
    }

    let _ = env_logger::try_init();
    log::info!("Starting ListenOS native backend for Electron");

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to create native backend runtime")
        .block_on(ipc::run());
}
