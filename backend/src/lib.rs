//! ListenOS native voice engine.
//!
//! The desktop shell owns presentation, window lifecycle, tray, deep links,
//! autostart, and updates. This crate stays focused on reusable audio,
//! transcription, persistence, shortcuts, and system automation. Native Rust
//! frontends call the typed exports directly.

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
mod notes;
mod shortcuts;
mod snippets;
mod streaming;
mod system;
mod transcription;
mod voice;

use std::{ops::Deref, sync::Arc};
use tokio::sync::Mutex;

pub use audio::{AudioDevice, AudioState};
pub use clipboard::{ClipboardContentType, ClipboardEntry, ClipboardService};
pub use commands::custom::{ActionStep, CustomCommand, CustomCommandsStore};
pub use commands::*;
pub use config::{
    AppConfig, DictationStyle, DictationStyleConfig, LanguagePreferences, ListeningMode,
    OverlayPosition, UIConfig, VibeActivationMode, VibeCodingConfig, VibeFormattingStyle,
};
pub use conversation::{ConversationMemory, ConversationStore, Fact, Message, Role};
pub use correction::CorrectionTracker;
pub use delivery::{DeliveryPhase, DeliveryState, DeliveryStatusSnapshot, TargetSurfaceKind};
pub use dictionary::{DictionaryStore, DictionaryWord};
pub use error_log::{ErrorEntry, ErrorLog, ErrorType};
pub use integrations::{AppIntegration, IntegrationAction, IntegrationInfo, IntegrationManager};
pub use notes::{Note, NotesStore};
pub use shortcuts::{ShortcutEvent, ShortcutService};
pub use snippets::{Snippet, SnippetsStore};
pub use streaming::{AudioHealthPhase, AudioRuntimeStatus, AudioStreamer, SAMPLE_RATE};
pub use transcription::{
    LocalModelInfo, TranscriptionComputeBackend, TranscriptionRuntimePhase,
    TranscriptionRuntimeStatus, TranscriptionService, TranscriptionSettings,
};
pub use voice::{
    ActionResult, ActionType, ConversationContext, VoiceContext, VoiceMode, VoiceRouter,
};

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

/// Shared backend state used by the native application core.
pub struct AppState {
    pub audio: Arc<Mutex<AudioState>>,
    pub config: Arc<Mutex<AppConfig>>,
    /// Serializes full-config transactions so native typed setters cannot lose
    /// unrelated concurrent updates or diverge in-memory state from disk.
    pub config_update_lock: Arc<Mutex<()>>,
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

        let persisted_config = AppConfig::load_from_disk();
        let mut app_config = persisted_config.clone().unwrap_or_default();
        // Legacy builds wrote these two sub-settings independently. Import them
        // only until the first authoritative full config exists; after that,
        // `config.json` wins so a stale legacy sidecar can never revert a newer
        // native setting after restart.
        if persisted_config.is_none() {
            if let Some(saved_languages) = config::LanguagePreferences::load_from_disk() {
                app_config.language_preferences = saved_languages;
            }
            if let Some(saved_vibe) = config::VibeCodingConfig::load_from_disk() {
                app_config.vibe_coding = saved_vibe;
            }
        }

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
