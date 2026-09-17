use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::{self, Receiver, Sender},
};
use std::time::Duration;

use futures_util::StreamExt;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::{mpsc as tokio_mpsc, watch};
use voice_os_lib::{
    AppConfig, AppState, AudioDevice, AudioRuntimeStatus, AudioState, ClipboardEntry,
    CustomCommand, DeliveryPhase, DictationStyle, DictationStyleConfig, DictionaryWord,
    IntegrationInfo, LocalModelInfo, Message, ShortcutEvent, ShortcutService, Snippet, State,
    TranscriptionRuntimeStatus, TranscriptionSettings, VibeCodingConfig, VoiceProcessingResult,
    add_dictionary_word, cancel_listening, cancel_pending_action, clear_conversation,
    confirm_pending_action, create_snippet, delete_custom_command, delete_dictionary_word,
    delete_snippet, download_local_model, get_audio_level, get_clipboard, get_clipboard_history,
    get_command_templates, get_config, get_conversation, get_custom_commands, get_dictionary_words,
    get_history, get_integrations, get_pending_action, get_snippets, get_status,
    install_command_template, new_conversation_session, normalize_hotkey_string,
    process_captured_audio, save_custom_command, set_assistant_hotkey, set_audio_device,
    set_clipboard, set_config, set_custom_command_enabled, set_integration_enabled,
    set_language_preferences, set_transcription_model, set_trigger_hotkey, set_vibe_coding_config,
    start_listening, take_capture_for_processing, update_dictionary_word, update_snippet,
    validate_hotkey_pair,
};

const AUDIO_LEVEL_INTERVAL: Duration = Duration::from_millis(45);
const STATUS_INTERVAL: Duration = Duration::from_millis(500);
const MICROPHONE_TEST_MAX_DURATION: Duration = Duration::from_secs(8);
const DASHBOARD_CLIPBOARD_HISTORY_LIMIT: usize = 50;
const UPDATE_MANIFEST_SCHEMA: u32 = 1;
const UPDATE_MANIFEST_MAX_BYTES: usize = 64 * 1024;
const UPDATE_ARTIFACT_MAX_BYTES: u64 = 1024 * 1024 * 1024;
const DEFAULT_UPDATE_MANIFEST_URL: &str =
    "https://pub-d6533f84d6a6405ab5f83d5a61d118ac.r2.dev/native-update.json";

#[derive(Debug, Clone)]
pub struct RuntimeStatusSnapshot {
    pub transcription: TranscriptionRuntimeStatus,
    pub audio: AudioRuntimeStatus,
    pub is_listening: bool,
    pub is_processing: bool,
    pub audio_device: Option<String>,
}

#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    Status(Box<RuntimeStatusSnapshot>),
    AudioLevel(f32),
    Idle,
    Listening {
        handsfree: bool,
    },
    Processing {
        handsfree: bool,
    },
    Success {
        transcription: String,
        summary: String,
    },
    ConfirmationRequired {
        transcription: String,
        summary: String,
    },
    Error(String),
}

#[derive(Debug, Clone)]
pub enum RuntimeUpdateEvent {
    Checking,
    UpToDate,
    Downloading { version: String },
    InstallerLaunched { version: String },
    DisabledInDevelopment,
    Failed { message: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseManifest {
    schema_version: u32,
    version: String,
    artifacts: std::collections::BTreeMap<String, ReleaseArtifact>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseArtifact {
    kind: String,
    arch: String,
    path: String,
    url: String,
    sha256: String,
}

#[derive(Debug, Clone)]
pub enum RuntimeMicrophoneTestEvent {
    Started,
    Level(f32),
    Finished { peak_level: f32 },
    Error(String),
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeConfigurationErrors {
    pub audio_devices: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfigurationSnapshot {
    pub config: AppConfig,
    pub audio_devices: Vec<AudioDevice>,
    pub local_models: Vec<LocalModelInfo>,
    pub transcription_settings: TranscriptionSettings,
    pub transcription_status: TranscriptionRuntimeStatus,
    pub command_templates: Vec<CustomCommand>,
    pub errors: RuntimeConfigurationErrors,
}

#[derive(Debug, Clone)]
pub enum RuntimeSettingsCommand {
    Refresh,
    SelectModel(String),
    DownloadModel(String),
    SelectMicrophone(String),
    MarkOnboardingComplete,
    SetTriggerHotkey(String),
    SetAssistantHotkey(String),
    SetLanguagePreferences {
        source_language: String,
        target_language: String,
    },
    SetVibeCodingConfig(VibeCodingConfig),
    SetUseGpu(bool),
    SetAutoStart(bool),
    InstallCommandTemplate(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeConfigurationOperation {
    Refresh,
    SelectModel,
    DownloadModel,
    SelectMicrophone,
    MarkOnboardingComplete,
    SetTriggerHotkey,
    SetAssistantHotkey,
    SetLanguagePreferences,
    SetVibeCodingConfig,
    SetUseGpu,
    SetAutoStart,
    InstallCommandTemplate,
}

impl RuntimeSettingsCommand {
    fn operation(&self) -> RuntimeConfigurationOperation {
        match self {
            Self::Refresh => RuntimeConfigurationOperation::Refresh,
            Self::SelectModel(_) => RuntimeConfigurationOperation::SelectModel,
            Self::DownloadModel(_) => RuntimeConfigurationOperation::DownloadModel,
            Self::SelectMicrophone(_) => RuntimeConfigurationOperation::SelectMicrophone,
            Self::MarkOnboardingComplete => RuntimeConfigurationOperation::MarkOnboardingComplete,
            Self::SetTriggerHotkey(_) => RuntimeConfigurationOperation::SetTriggerHotkey,
            Self::SetAssistantHotkey(_) => RuntimeConfigurationOperation::SetAssistantHotkey,
            Self::SetLanguagePreferences { .. } => {
                RuntimeConfigurationOperation::SetLanguagePreferences
            }
            Self::SetVibeCodingConfig(_) => RuntimeConfigurationOperation::SetVibeCodingConfig,
            Self::SetUseGpu(_) => RuntimeConfigurationOperation::SetUseGpu,
            Self::SetAutoStart(_) => RuntimeConfigurationOperation::SetAutoStart,
            Self::InstallCommandTemplate(_) => {
                RuntimeConfigurationOperation::InstallCommandTemplate
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum RuntimeConfigurationEvent {
    Snapshot(Box<RuntimeConfigurationSnapshot>),
    OperationStarted(RuntimeConfigurationOperation),
    OperationFinished(RuntimeConfigurationOperation),
    OperationFailed {
        operation: RuntimeConfigurationOperation,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct RuntimeDashboardSnapshot {
    pub history: Vec<VoiceProcessingResult>,
    pub conversation: Vec<Message>,
    pub clipboard: String,
    pub clipboard_history: Vec<ClipboardEntry>,
    pub integrations: Vec<IntegrationInfo>,
    pub custom_commands: Vec<CustomCommand>,
    pub command_templates: Vec<CustomCommand>,
    pub dictionary_words: Vec<DictionaryWord>,
    pub snippets: Vec<Snippet>,
    pub dictation_style: DictationStyleConfig,
}

#[derive(Debug, Clone)]
pub enum RuntimeDashboardEvent {
    RefreshStarted,
    Snapshot(Box<RuntimeDashboardSnapshot>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeDictationStyleContext {
    Personal,
    Work,
    Email,
    Other,
}

#[derive(Debug, Clone)]
pub enum RuntimeDashboardMutation {
    ClearConversation,
    NewConversationSession,
    SetClipboard(String),
    SetIntegrationEnabled {
        name: String,
        enabled: bool,
    },
    SetCustomCommandEnabled {
        id: String,
        enabled: bool,
    },
    ImportCustomCommands(Vec<CustomCommand>),
    SaveCustomCommand(CustomCommand),
    DeleteCustomCommand(String),
    AddDictionaryWord {
        word: String,
        is_auto_learned: bool,
    },
    UpdateDictionaryWord {
        id: String,
        word: String,
        phonetic: Option<String>,
    },
    DeleteDictionaryWord(String),
    CreateSnippet {
        trigger: String,
        expansion: String,
    },
    UpdateSnippet {
        id: String,
        trigger: String,
        expansion: String,
    },
    DeleteSnippet(String),
    SetDictationStyle {
        context: RuntimeDictationStyleContext,
        style: DictationStyle,
    },
}

impl RuntimeDashboardMutation {
    fn modifies_config(&self) -> bool {
        matches!(self, Self::SetDictationStyle { .. })
    }
}

#[derive(Debug, Clone)]
pub enum RuntimeDashboardMutationEvent {
    Started,
    Succeeded { message: String },
    Failed { message: String },
}

#[derive(Debug)]
enum ControllerCommand {
    Shortcut(ShortcutEvent),
    Start { handsfree: bool },
    Stop { dictation_only: bool },
    CancelCapture,
    ConfirmPending,
    CancelPending,
    Settings(RuntimeSettingsCommand),
    RefreshDashboard,
    DashboardMutation(RuntimeDashboardMutation),
    StartMicrophoneTest,
    FinishMicrophoneTest { generation: Option<u64> },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CaptureMode {
    #[default]
    Idle,
    Trigger,
    Handsfree,
    MicrophoneTest,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AudioLevelMode {
    #[default]
    Idle,
    Normal,
    MicrophoneTest,
}

struct CommandLoopContext {
    state: Arc<AppState>,
    events: Sender<RuntimeEvent>,
    configuration_events: Sender<RuntimeConfigurationEvent>,
    configuration_snapshot: Arc<Mutex<Option<RuntimeConfigurationSnapshot>>>,
    dashboard_events: Sender<RuntimeDashboardEvent>,
    dashboard_mutation_events: Sender<RuntimeDashboardMutationEvent>,
    dashboard_snapshot: Arc<Mutex<Option<RuntimeDashboardSnapshot>>>,
    dashboard_refreshes: Arc<tokio::sync::Mutex<()>>,
    microphone_test_events: Sender<RuntimeMicrophoneTestEvent>,
    settings_updates: Arc<tokio::sync::Mutex<()>>,
    audio_level_mode: watch::Sender<AudioLevelMode>,
    command_dispatch: tokio_mpsc::UnboundedSender<ControllerCommand>,
    microphone_test_active: Arc<AtomicBool>,
    microphone_test_peak: Arc<AtomicU32>,
}

fn create_shortcut_service(
    trigger: &str,
    assistant: &str,
    dispatch: &tokio_mpsc::UnboundedSender<ControllerCommand>,
) -> Result<ShortcutService, String> {
    let dispatch = dispatch.clone();
    ShortcutService::new(trigger, assistant, move |event| {
        let _ = dispatch.send(ControllerCommand::Shortcut(event));
    })
}

#[derive(Clone, Debug)]
struct ShortcutPair {
    trigger: String,
    assistant: String,
}

#[derive(Clone, Debug)]
struct PendingShortcutUpdate {
    operation: RuntimeConfigurationOperation,
    previous: ShortcutPair,
}

pub struct RuntimeController {
    _state: Arc<AppState>,
    events: Arc<Mutex<Receiver<RuntimeEvent>>>,
    update_events: Arc<Mutex<Receiver<RuntimeUpdateEvent>>>,
    microphone_test_events: Arc<Mutex<Receiver<RuntimeMicrophoneTestEvent>>>,
    configuration_events: Arc<Mutex<Receiver<RuntimeConfigurationEvent>>>,
    configuration_snapshot: Arc<Mutex<Option<RuntimeConfigurationSnapshot>>>,
    dashboard_events: Arc<Mutex<Receiver<RuntimeDashboardEvent>>>,
    dashboard_mutation_events: Arc<Mutex<Receiver<RuntimeDashboardMutationEvent>>>,
    dashboard_snapshot: Arc<Mutex<Option<RuntimeDashboardSnapshot>>>,
    commands: tokio_mpsc::UnboundedSender<ControllerCommand>,
    shortcuts: Option<ShortcutService>,
    shortcut_pair: ShortcutPair,
    pending_shortcut_update: Option<PendingShortcutUpdate>,
    update_event_tx: Sender<RuntimeUpdateEvent>,
    update_busy: Arc<AtomicBool>,
    _runtime: Runtime,
}

impl RuntimeController {
    pub fn new() -> Result<Self, String> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .thread_name("listenos-native-runtime")
            .build()
            .map_err(|error| format!("Failed to create native runtime: {error}"))?;
        let state = Arc::new(AppState::default());
        let (event_tx, event_rx) = mpsc::channel::<RuntimeEvent>();
        let events = Arc::new(Mutex::new(event_rx));
        let (update_event_tx, update_event_rx) = mpsc::channel::<RuntimeUpdateEvent>();
        let update_events = Arc::new(Mutex::new(update_event_rx));
        let update_busy = Arc::new(AtomicBool::new(false));
        let (microphone_test_event_tx, microphone_test_event_rx) =
            mpsc::channel::<RuntimeMicrophoneTestEvent>();
        let microphone_test_events = Arc::new(Mutex::new(microphone_test_event_rx));
        let (configuration_event_tx, configuration_event_rx) =
            mpsc::channel::<RuntimeConfigurationEvent>();
        let configuration_events = Arc::new(Mutex::new(configuration_event_rx));
        let configuration_snapshot = Arc::new(Mutex::new(None));
        let (dashboard_event_tx, dashboard_event_rx) = mpsc::channel::<RuntimeDashboardEvent>();
        let dashboard_events = Arc::new(Mutex::new(dashboard_event_rx));
        let (dashboard_mutation_event_tx, dashboard_mutation_event_rx) =
            mpsc::channel::<RuntimeDashboardMutationEvent>();
        let dashboard_mutation_events = Arc::new(Mutex::new(dashboard_mutation_event_rx));
        let dashboard_snapshot = Arc::new(Mutex::new(None));
        let (command_tx, command_rx) = tokio_mpsc::unbounded_channel::<ControllerCommand>();
        let (audio_level_mode_tx, audio_level_mode_rx) = watch::channel(AudioLevelMode::Idle);
        let microphone_test_active = Arc::new(AtomicBool::new(false));
        let microphone_test_peak = Arc::new(AtomicU32::new(0.0_f32.to_bits()));

        let (trigger, assistant) = runtime.block_on(async {
            let config = state.config.lock().await;
            (
                config.trigger_hotkey.clone(),
                config.assistant_hotkey.clone(),
            )
        });
        let settings_updates = Arc::new(tokio::sync::Mutex::new(()));
        let dashboard_refreshes = Arc::new(tokio::sync::Mutex::new(()));

        runtime.spawn(run_command_loop(
            command_rx,
            CommandLoopContext {
                state: state.clone(),
                events: event_tx.clone(),
                configuration_events: configuration_event_tx,
                configuration_snapshot: configuration_snapshot.clone(),
                dashboard_events: dashboard_event_tx,
                dashboard_mutation_events: dashboard_mutation_event_tx,
                dashboard_snapshot: dashboard_snapshot.clone(),
                dashboard_refreshes,
                microphone_test_events: microphone_test_event_tx.clone(),
                settings_updates,
                audio_level_mode: audio_level_mode_tx,
                command_dispatch: command_tx.clone(),
                microphone_test_active: microphone_test_active.clone(),
                microphone_test_peak: microphone_test_peak.clone(),
            },
        ));
        runtime.spawn(run_audio_level_loop(
            state.clone(),
            event_tx.clone(),
            microphone_test_event_tx,
            audio_level_mode_rx,
            microphone_test_peak,
        ));
        runtime.spawn(run_status_loop(
            state.clone(),
            event_tx,
            microphone_test_active,
        ));
        let prewarm_transcription = state.transcription.clone();
        runtime.spawn(async move {
            if prewarm_transcription.runtime_status().model_downloaded
                && let Err(error) = prewarm_transcription.preload_selected_model().await
            {
                eprintln!("ListenOS model prewarm failed: {error}");
            }
        });
        let _ = command_tx.send(ControllerCommand::Settings(RuntimeSettingsCommand::Refresh));
        let _ = command_tx.send(ControllerCommand::RefreshDashboard);
        let _ = queue_update_check(&runtime, update_event_tx.clone(), update_busy.clone());

        Ok(Self {
            _state: state,
            events,
            update_events,
            microphone_test_events,
            configuration_events,
            configuration_snapshot,
            dashboard_events,
            dashboard_mutation_events,
            dashboard_snapshot,
            commands: command_tx,
            shortcuts: None,
            shortcut_pair: ShortcutPair { trigger, assistant },
            pending_shortcut_update: None,
            update_event_tx,
            update_busy,
            _runtime: runtime,
        })
    }

    /// Register global shortcuts on the GPUI/platform event-loop thread.
    /// `global-hotkey` requires manager creation and mutation to stay on that
    /// owner thread on Windows and macOS; only its event receiver runs on the
    /// service's internal worker thread.
    pub fn initialize_shortcuts(&mut self) -> Result<(), String> {
        if self.shortcuts.is_some() {
            return Ok(());
        }
        let service = create_shortcut_service(
            &self.shortcut_pair.trigger,
            &self.shortcut_pair.assistant,
            &self.commands,
        )
        .map_err(|error| format!("Global shortcuts unavailable: {error}"))?;
        self.shortcuts = Some(service);
        Ok(())
    }

    pub fn events(&self) -> Arc<Mutex<Receiver<RuntimeEvent>>> {
        self.events.clone()
    }

    pub fn update_events(&self) -> Arc<Mutex<Receiver<RuntimeUpdateEvent>>> {
        self.update_events.clone()
    }

    pub fn check_for_updates(&self) -> Result<(), String> {
        queue_update_check(
            &self._runtime,
            self.update_event_tx.clone(),
            self.update_busy.clone(),
        )
    }

    pub fn microphone_test_events(&self) -> Arc<Mutex<Receiver<RuntimeMicrophoneTestEvent>>> {
        self.microphone_test_events.clone()
    }

    pub fn configuration_events(&self) -> Arc<Mutex<Receiver<RuntimeConfigurationEvent>>> {
        self.configuration_events.clone()
    }

    pub fn configuration_snapshot(&self) -> Option<RuntimeConfigurationSnapshot> {
        self.configuration_snapshot
            .lock()
            .ok()
            .and_then(|snapshot| snapshot.clone())
    }

    pub fn dashboard_events(&self) -> Arc<Mutex<Receiver<RuntimeDashboardEvent>>> {
        self.dashboard_events.clone()
    }

    pub fn dashboard_mutation_events(&self) -> Arc<Mutex<Receiver<RuntimeDashboardMutationEvent>>> {
        self.dashboard_mutation_events.clone()
    }

    pub fn dashboard_snapshot(&self) -> Option<RuntimeDashboardSnapshot> {
        self.dashboard_snapshot
            .lock()
            .ok()
            .and_then(|snapshot| snapshot.clone())
    }

    pub fn refresh_dashboard_data(&self) -> Result<(), String> {
        self.commands
            .send(ControllerCommand::RefreshDashboard)
            .map_err(|_| "Native runtime command loop is unavailable".to_string())
    }

    pub fn send_dashboard_mutation(
        &self,
        mutation: RuntimeDashboardMutation,
    ) -> Result<(), String> {
        self.commands
            .send(ControllerCommand::DashboardMutation(mutation))
            .map_err(|_| "Native runtime command loop is unavailable".to_string())
    }

    pub fn clear_conversation(&self) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::ClearConversation)
    }

    pub fn new_conversation_session(&self) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::NewConversationSession)
    }

    pub fn set_clipboard(&self, content: impl Into<String>) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::SetClipboard(content.into()))
    }

    pub fn set_integration_enabled(
        &self,
        name: impl Into<String>,
        enabled: bool,
    ) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::SetIntegrationEnabled {
            name: name.into(),
            enabled,
        })
    }

    pub fn set_custom_command_enabled(
        &self,
        id: impl Into<String>,
        enabled: bool,
    ) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::SetCustomCommandEnabled {
            id: id.into(),
            enabled,
        })
    }

    pub fn save_custom_command(&self, command: CustomCommand) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::SaveCustomCommand(command))
    }

    pub fn import_custom_commands(&self, commands: Vec<CustomCommand>) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::ImportCustomCommands(commands))
    }

    pub fn delete_custom_command(&self, id: impl Into<String>) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::DeleteCustomCommand(id.into()))
    }

    pub fn add_dictionary_word(
        &self,
        word: impl Into<String>,
        is_auto_learned: bool,
    ) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::AddDictionaryWord {
            word: word.into(),
            is_auto_learned,
        })
    }

    pub fn update_dictionary_word(
        &self,
        id: impl Into<String>,
        word: impl Into<String>,
        phonetic: Option<String>,
    ) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::UpdateDictionaryWord {
            id: id.into(),
            word: word.into(),
            phonetic,
        })
    }

    pub fn delete_dictionary_word(&self, id: impl Into<String>) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::DeleteDictionaryWord(id.into()))
    }

    pub fn create_snippet(
        &self,
        trigger: impl Into<String>,
        expansion: impl Into<String>,
    ) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::CreateSnippet {
            trigger: trigger.into(),
            expansion: expansion.into(),
        })
    }

    pub fn update_snippet(
        &self,
        id: impl Into<String>,
        trigger: impl Into<String>,
        expansion: impl Into<String>,
    ) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::UpdateSnippet {
            id: id.into(),
            trigger: trigger.into(),
            expansion: expansion.into(),
        })
    }

    pub fn delete_snippet(&self, id: impl Into<String>) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::DeleteSnippet(id.into()))
    }

    pub fn set_dictation_style(
        &self,
        context: RuntimeDictationStyleContext,
        style: DictationStyle,
    ) -> Result<(), String> {
        self.send_dashboard_mutation(RuntimeDashboardMutation::SetDictationStyle { context, style })
    }

    pub fn send_settings_command(&self, command: RuntimeSettingsCommand) -> Result<(), String> {
        self.commands
            .send(ControllerCommand::Settings(command))
            .map_err(|_| "Native runtime command loop is unavailable".to_string())
    }

    pub fn select_model(&self, model: impl Into<String>) -> Result<(), String> {
        self.send_settings_command(RuntimeSettingsCommand::SelectModel(model.into()))
    }

    pub fn download_model(&self, model: impl Into<String>) -> Result<(), String> {
        self.send_settings_command(RuntimeSettingsCommand::DownloadModel(model.into()))
    }

    pub fn select_microphone(&self, device: impl Into<String>) -> Result<(), String> {
        self.send_settings_command(RuntimeSettingsCommand::SelectMicrophone(device.into()))
    }

    pub fn mark_onboarding_complete(&self) -> Result<(), String> {
        self.send_settings_command(RuntimeSettingsCommand::MarkOnboardingComplete)
    }

    pub fn set_trigger_hotkey(&mut self, hotkey: impl Into<String>) -> Result<(), String> {
        self.begin_shortcut_update(
            RuntimeConfigurationOperation::SetTriggerHotkey,
            hotkey.into(),
        )
    }

    pub fn set_assistant_hotkey(&mut self, hotkey: impl Into<String>) -> Result<(), String> {
        self.begin_shortcut_update(
            RuntimeConfigurationOperation::SetAssistantHotkey,
            hotkey.into(),
        )
    }

    fn begin_shortcut_update(
        &mut self,
        operation: RuntimeConfigurationOperation,
        hotkey: String,
    ) -> Result<(), String> {
        if self.pending_shortcut_update.is_some() {
            return Err("A shortcut change is already being saved.".to_string());
        }

        let normalized = normalize_hotkey_string(&hotkey)?;
        let previous = self.shortcut_pair.clone();
        let mut next = previous.clone();
        let command = match operation {
            RuntimeConfigurationOperation::SetTriggerHotkey => {
                next.trigger = normalized.clone();
                RuntimeSettingsCommand::SetTriggerHotkey(normalized.clone())
            }
            RuntimeConfigurationOperation::SetAssistantHotkey => {
                next.assistant = normalized.clone();
                RuntimeSettingsCommand::SetAssistantHotkey(normalized.clone())
            }
            _ => return Err("Unsupported hotkey update".to_string()),
        };
        validate_hotkey_pair(&next.trigger, &next.assistant)?;

        let shortcuts = self
            .shortcuts
            .as_mut()
            .ok_or_else(|| "Global shortcuts are unavailable in this session.".to_string())?;
        shortcuts
            .update(&next.trigger, &next.assistant)
            .map_err(|error| format!("Could not register shortcut {normalized}: {error}"))?;
        self.shortcut_pair = next;
        self.pending_shortcut_update = Some(PendingShortcutUpdate {
            operation,
            previous: previous.clone(),
        });

        if let Err(error) = self.send_settings_command(command) {
            let rollback = self
                .shortcuts
                .as_mut()
                .expect("shortcut service existed before persistence enqueue")
                .update(&previous.trigger, &previous.assistant);
            self.pending_shortcut_update = None;
            self.shortcut_pair = previous;
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => {
                    self.shortcuts = None;
                    Err(format!(
                        "{error}; failed to restore previous OS shortcut registration: {rollback_error}. Global shortcuts were disabled for this session."
                    ))
                }
            };
        }
        Ok(())
    }

    pub fn reconcile_shortcut_configuration_event(
        &mut self,
        event: &mut RuntimeConfigurationEvent,
    ) {
        let event_operation = match event {
            RuntimeConfigurationEvent::OperationFinished(operation) => Some(*operation),
            RuntimeConfigurationEvent::OperationFailed { operation, .. } => Some(*operation),
            _ => None,
        };
        let Some(pending) = self.pending_shortcut_update.clone() else {
            return;
        };
        if event_operation != Some(pending.operation) {
            return;
        }

        match event {
            RuntimeConfigurationEvent::OperationFinished(_) => {
                self.pending_shortcut_update = None;
            }
            RuntimeConfigurationEvent::OperationFailed { message, .. } => {
                let rollback = self.shortcuts.as_mut().map(|service| {
                    service.update(&pending.previous.trigger, &pending.previous.assistant)
                });
                self.pending_shortcut_update = None;
                self.shortcut_pair = pending.previous;
                match rollback {
                    Some(Ok(())) => {}
                    Some(Err(rollback_error)) => {
                        self.shortcuts = None;
                        message.push_str(&format!(
                            "; failed to restore previous OS shortcut registration: {rollback_error}. Global shortcuts were disabled for this session."
                        ));
                    }
                    None => {
                        message.push_str("; global shortcut service is unavailable for rollback.");
                    }
                }
            }
            _ => {}
        }
    }

    pub fn set_language_preferences(
        &self,
        source_language: impl Into<String>,
        target_language: impl Into<String>,
    ) -> Result<(), String> {
        self.send_settings_command(RuntimeSettingsCommand::SetLanguagePreferences {
            source_language: source_language.into(),
            target_language: target_language.into(),
        })
    }

    pub fn set_vibe_coding_config(&self, config: VibeCodingConfig) -> Result<(), String> {
        self.send_settings_command(RuntimeSettingsCommand::SetVibeCodingConfig(config))
    }

    pub fn set_use_gpu(&self, enabled: bool) -> Result<(), String> {
        self.send_settings_command(RuntimeSettingsCommand::SetUseGpu(enabled))
    }

    pub fn set_auto_start(&self, enabled: bool) -> Result<(), String> {
        self.send_settings_command(RuntimeSettingsCommand::SetAutoStart(enabled))
    }

    pub fn install_command_template(&self, template_id: impl Into<String>) -> Result<(), String> {
        self.commands
            .send(ControllerCommand::Settings(
                RuntimeSettingsCommand::InstallCommandTemplate(template_id.into()),
            ))
            .map_err(|_| "Native runtime command loop is unavailable".to_string())
    }

    pub fn start_microphone_test(&self) -> Result<(), String> {
        self.commands
            .send(ControllerCommand::StartMicrophoneTest)
            .map_err(|_| "Native runtime is not available".to_string())
    }

    pub fn stop_capture(&self, dictation_only: bool) -> Result<(), String> {
        self.commands
            .send(ControllerCommand::Stop { dictation_only })
            .map_err(|_| "Native runtime is not available".to_string())
    }

    pub fn cancel_capture(&self) -> Result<(), String> {
        self.commands
            .send(ControllerCommand::CancelCapture)
            .map_err(|_| "Native runtime is not available".to_string())
    }

    pub fn confirm_pending(&self) -> Result<(), String> {
        self.commands
            .send(ControllerCommand::ConfirmPending)
            .map_err(|_| "Native runtime is not available".to_string())
    }

    pub fn cancel_pending(&self) -> Result<(), String> {
        self.commands
            .send(ControllerCommand::CancelPending)
            .map_err(|_| "Native runtime is not available".to_string())
    }
}

fn queue_update_check(
    runtime: &Runtime,
    events: Sender<RuntimeUpdateEvent>,
    busy: Arc<AtomicBool>,
) -> Result<(), String> {
    if cfg!(debug_assertions) {
        let _ = events.send(RuntimeUpdateEvent::DisabledInDevelopment);
        return Ok(());
    }
    busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "An update check is already running.".to_string())?;
    let _ = events.send(RuntimeUpdateEvent::Checking);
    runtime.spawn(async move {
        let result = run_update_check(&events).await;
        busy.store(false, Ordering::Release);
        if let Err(message) = result {
            let _ = events.send(RuntimeUpdateEvent::Failed { message });
        }
    });
    Ok(())
}

fn update_manifest_url() -> &'static str {
    option_env!("LISTENOS_UPDATE_MANIFEST_URL").unwrap_or(DEFAULT_UPDATE_MANIFEST_URL)
}

fn parse_https_url(raw: &str, label: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|error| format!("Invalid {label} URL: {error}"))?;
    if url.scheme() != "https" {
        return Err(format!("{label} URL must use HTTPS."));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{label} URL must not contain credentials."));
    }
    Ok(url)
}

fn normalized_sha256(raw: &str) -> Result<String, String> {
    if raw.len() != 64
        || !raw
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(
            "Update artifact SHA-256 must be exactly 64 lowercase hexadecimal characters."
                .to_string(),
        );
    }
    Ok(raw.to_string())
}

fn select_release_artifact(
    manifest: ReleaseManifest,
    current_version: &str,
    os: &str,
    arch: &str,
) -> Result<Option<(Version, ReleaseArtifact)>, String> {
    if manifest.schema_version != UPDATE_MANIFEST_SCHEMA {
        return Err(format!(
            "Unsupported update manifest schema {} (expected {}).",
            manifest.schema_version, UPDATE_MANIFEST_SCHEMA
        ));
    }
    if manifest.artifacts.len() != 2
        || !manifest.artifacts.contains_key("windows")
        || !manifest.artifacts.contains_key("macos")
    {
        return Err(
            "Update manifest artifacts must contain exactly 'windows' and 'macos'.".to_string(),
        );
    }
    let current = Version::parse(current_version)
        .map_err(|error| format!("Invalid current ListenOS version {current_version}: {error}"))?;
    let available = Version::parse(&manifest.version).map_err(|error| {
        format!(
            "Invalid update manifest version {}: {error}",
            manifest.version
        )
    })?;
    if available <= current {
        return Ok(None);
    }
    let (artifact_key, expected_kind, expected_arch, expected_filename) = match (os, arch) {
        ("windows", "x86_64") => (
            "windows",
            "nsis-installer",
            "x86_64",
            format!("ListenOS-{available}-Setup-x86_64.exe"),
        ),
        ("macos", "aarch64") => (
            "macos",
            "dmg-installer",
            "universal",
            format!("ListenOS-{available}-macos-universal.dmg"),
        ),
        ("macos", "x86_64") => (
            "macos",
            "dmg-installer",
            "universal",
            format!("ListenOS-{available}-macos-universal.dmg"),
        ),
        _ => return Err(format!("Native updates are not supported on {os}/{arch}.")),
    };
    let artifact = manifest
        .artifacts
        .get(artifact_key)
        .cloned()
        .ok_or_else(|| format!("Update {available} does not contain a {artifact_key} artifact."))?;
    if artifact.kind != expected_kind {
        return Err(format!(
            "Update artifact kind '{}' is invalid for {os}; expected '{expected_kind}'.",
            artifact.kind
        ));
    }
    if artifact.arch != expected_arch {
        return Err(format!(
            "Update artifact architecture '{}' does not match this {arch} build.",
            artifact.arch
        ));
    }
    normalized_sha256(&artifact.sha256)?;
    let artifact_url = parse_https_url(&artifact.url, "update artifact")?;
    if artifact.path != expected_filename {
        return Err(format!(
            "Update artifact path '{}' does not match the shipping filename '{expected_filename}'.",
            artifact.path
        ));
    }
    let url_filename = artifact_url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default();
    if url_filename != artifact.path {
        return Err("Update artifact URL filename does not match its manifest path.".to_string());
    }
    let immutable_suffix = format!("/releases/v{available}/{}", artifact.path);
    if !artifact_url.path().ends_with(&immutable_suffix) {
        return Err(format!(
            "Update artifact URL must point at the immutable release path '{immutable_suffix}'."
        ));
    }
    Ok(Some((available, artifact)))
}

async fn run_update_check(events: &Sender<RuntimeUpdateEvent>) -> Result<(), String> {
    let manifest_url = parse_https_url(update_manifest_url(), "update manifest")?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(15 * 60))
        .user_agent(format!(
            "ListenOS/{} native-updater",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(|error| format!("Create update client: {error}"))?;
    let response = client
        .get(manifest_url)
        .send()
        .await
        .map_err(|error| format!("Fetch update manifest: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Fetch update manifest: {error}"))?;
    if response.url().scheme() != "https" {
        return Err("Update manifest redirected to a non-HTTPS URL.".to_string());
    }
    if response
        .content_length()
        .is_some_and(|length| length > UPDATE_MANIFEST_MAX_BYTES as u64)
    {
        return Err("Update manifest is larger than 64 KiB.".to_string());
    }
    let manifest_bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Read update manifest: {error}"))?;
    if manifest_bytes.len() > UPDATE_MANIFEST_MAX_BYTES {
        return Err("Update manifest is larger than 64 KiB.".to_string());
    }
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("Parse update manifest: {error}"))?;
    let Some((version, artifact)) = select_release_artifact(
        manifest,
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )?
    else {
        let _ = events.send(RuntimeUpdateEvent::UpToDate);
        return Ok(());
    };
    let version_string = version.to_string();
    let _ = events.send(RuntimeUpdateEvent::Downloading {
        version: version_string.clone(),
    });
    let package = download_verified_update(&client, &version, &artifact).await?;
    launch_verified_update(&package)?;
    let _ = events.send(RuntimeUpdateEvent::InstallerLaunched {
        version: version_string,
    });
    Ok(())
}

async fn download_verified_update(
    client: &reqwest::Client,
    version: &Version,
    artifact: &ReleaseArtifact,
) -> Result<std::path::PathBuf, String> {
    let url = parse_https_url(&artifact.url, "update artifact")?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("Download ListenOS {version}: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Download ListenOS {version}: {error}"))?;
    if response.url().scheme() != "https" {
        return Err("Update artifact redirected to a non-HTTPS URL.".to_string());
    }
    if response
        .content_length()
        .is_some_and(|length| length == 0 || length > UPDATE_ARTIFACT_MAX_BYTES)
    {
        return Err("Update artifact is empty or exceeds the 1 GiB safety limit.".to_string());
    }

    let extension = std::path::Path::new(&artifact.path)
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| "Update artifact path has no file extension.".to_string())?;
    let path = std::env::temp_dir().join(format!(
        "ListenOS-update-{version}-{}.{}",
        uuid::Uuid::new_v4(),
        extension
    ));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
        .map_err(|error| format!("Create update package {}: {error}", path.display()))?;
    let mut stream = response.bytes_stream();
    let mut hasher = Sha256::new();
    let mut downloaded = 0_u64;
    let download_result: Result<(), String> = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| format!("Download ListenOS {version}: {error}"))?;
            downloaded = downloaded
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| "Update artifact size overflowed.".to_string())?;
            if downloaded > UPDATE_ARTIFACT_MAX_BYTES {
                return Err("Downloaded update exceeded the 1 GiB safety limit.".to_string());
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|error| format!("Write update package: {error}"))?;
        }
        file.flush()
            .await
            .map_err(|error| format!("Flush update package: {error}"))?;
        if downloaded == 0 {
            return Err("Downloaded update artifact is empty.".to_string());
        }
        let actual_hash = format!("{:x}", hasher.finalize());
        let expected_hash = normalized_sha256(&artifact.sha256)?;
        if actual_hash != expected_hash {
            return Err("Downloaded update failed SHA-256 verification.".to_string());
        }
        Ok(())
    }
    .await;
    drop(file);
    if let Err(error) = download_result {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error);
    }
    Ok(path)
}

fn launch_verified_update(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new(path)
            .spawn()
            .map_err(|error| format!("Launch verified ListenOS installer: {error}"))?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg(path)
            .spawn()
            .map_err(|error| format!("Open verified ListenOS disk image: {error}"))?;
        Ok(())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = path;
        Err("Native update installation is unsupported on this platform.".to_string())
    }
}

async fn run_command_loop(
    mut commands: tokio_mpsc::UnboundedReceiver<ControllerCommand>,
    context: CommandLoopContext,
) {
    let CommandLoopContext {
        state,
        events,
        configuration_events,
        configuration_snapshot,
        dashboard_events,
        dashboard_mutation_events,
        dashboard_snapshot,
        dashboard_refreshes,
        microphone_test_events,
        settings_updates,
        audio_level_mode,
        command_dispatch,
        microphone_test_active,
        microphone_test_peak,
    } = context;
    let mut capture_mode = CaptureMode::Idle;
    let mut microphone_test_generation = 0_u64;

    while let Some(command) = commands.recv().await {
        let command = match command {
            ControllerCommand::Shortcut(ShortcutEvent::TriggerPressed) => {
                ControllerCommand::Start { handsfree: false }
            }
            ControllerCommand::Shortcut(ShortcutEvent::TriggerReleased) => {
                if capture_mode != CaptureMode::Trigger {
                    continue;
                }
                ControllerCommand::Stop {
                    dictation_only: false,
                }
            }
            ControllerCommand::Shortcut(ShortcutEvent::AssistantPressed) => match capture_mode {
                CaptureMode::Idle => ControllerCommand::Start { handsfree: true },
                CaptureMode::Handsfree => ControllerCommand::Stop {
                    dictation_only: true,
                },
                CaptureMode::Trigger | CaptureMode::MicrophoneTest => continue,
            },
            command => command,
        };

        match command {
            ControllerCommand::Start { handsfree } if capture_mode == CaptureMode::Idle => {
                let _ = events.send(RuntimeEvent::Listening { handsfree });
                match start_listening(State::new(state.as_ref())).await {
                    Ok(_) => {
                        capture_mode = if handsfree {
                            CaptureMode::Handsfree
                        } else {
                            CaptureMode::Trigger
                        };
                        let _ = audio_level_mode.send(AudioLevelMode::Normal);
                    }
                    Err(error) => {
                        let _ = audio_level_mode.send(AudioLevelMode::Idle);
                        let _ = events.send(RuntimeEvent::Error(error));
                    }
                }
            }
            ControllerCommand::Stop { dictation_only }
                if matches!(capture_mode, CaptureMode::Trigger | CaptureMode::Handsfree) =>
            {
                let handsfree = capture_mode == CaptureMode::Handsfree;
                // Snapshot the capture synchronously (fast: flag flips + sample
                // clone) so a subsequent shortcut press is never queued behind
                // a multi-second transcription. The slow stages run on a
                // background task and report back through the event channel.
                let captured = take_capture_for_processing(State::new(state.as_ref())).await;
                capture_mode = CaptureMode::Idle;
                let _ = audio_level_mode.send(AudioLevelMode::Idle);
                match captured {
                    Err(error) => {
                        emit_processing_result(state.as_ref(), Err(error), &events).await;
                    }
                    Ok(captured) => {
                        let _ = events.send(RuntimeEvent::Processing { handsfree });
                        let task_state = state.clone();
                        let task_events = events.clone();
                        tokio::spawn(async move {
                            let result = process_captured_audio(
                                State::new(task_state.as_ref()),
                                captured,
                                Some(dictation_only),
                            )
                            .await;
                            emit_processing_result(task_state.as_ref(), result, &task_events).await;
                        });
                    }
                }
            }
            ControllerCommand::CancelCapture
                if matches!(capture_mode, CaptureMode::Trigger | CaptureMode::Handsfree) =>
            {
                match cancel_listening(State::new(state.as_ref())).await {
                    Ok(_) => {
                        capture_mode = CaptureMode::Idle;
                        let _ = audio_level_mode.send(AudioLevelMode::Idle);
                        let _ = events.send(RuntimeEvent::Idle);
                    }
                    Err(error) => {
                        let _ = events.send(RuntimeEvent::Error(error));
                    }
                }
            }
            ControllerCommand::StartMicrophoneTest if capture_mode == CaptureMode::Idle => {
                microphone_test_generation = microphone_test_generation.wrapping_add(1);
                let generation = microphone_test_generation;
                microphone_test_peak.store(0.0_f32.to_bits(), Ordering::Release);
                match start_listening(State::new(state.as_ref())).await {
                    Ok(_) => {
                        capture_mode = CaptureMode::MicrophoneTest;
                        microphone_test_active.store(true, Ordering::Release);
                        let _ = audio_level_mode.send(AudioLevelMode::MicrophoneTest);
                        let _ = microphone_test_events.send(RuntimeMicrophoneTestEvent::Started);
                        let dispatch = command_dispatch.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(MICROPHONE_TEST_MAX_DURATION).await;
                            let _ = dispatch.send(ControllerCommand::FinishMicrophoneTest {
                                generation: Some(generation),
                            });
                        });
                    }
                    Err(error) => {
                        microphone_test_active.store(false, Ordering::Release);
                        let _ = audio_level_mode.send(AudioLevelMode::Idle);
                        let _ =
                            microphone_test_events.send(RuntimeMicrophoneTestEvent::Error(error));
                    }
                }
            }
            ControllerCommand::FinishMicrophoneTest { generation }
                if capture_mode == CaptureMode::MicrophoneTest
                    && generation.is_none_or(|expected| expected == microphone_test_generation) =>
            {
                let result = cancel_listening(State::new(state.as_ref())).await;
                capture_mode = CaptureMode::Idle;
                microphone_test_active.store(false, Ordering::Release);
                let _ = audio_level_mode.send(AudioLevelMode::Idle);
                match result {
                    Ok(_) => {
                        let peak_level = f32::from_bits(
                            microphone_test_peak.swap(0.0_f32.to_bits(), Ordering::AcqRel),
                        );
                        let _ = microphone_test_events
                            .send(RuntimeMicrophoneTestEvent::Finished { peak_level });
                    }
                    Err(error) => {
                        let _ =
                            microphone_test_events.send(RuntimeMicrophoneTestEvent::Error(error));
                    }
                }
            }
            ControllerCommand::CancelCapture if capture_mode == CaptureMode::MicrophoneTest => {
                let result = cancel_listening(State::new(state.as_ref())).await;
                capture_mode = CaptureMode::Idle;
                microphone_test_active.store(false, Ordering::Release);
                let _ = audio_level_mode.send(AudioLevelMode::Idle);
                match result {
                    Ok(_) => {
                        let peak_level = f32::from_bits(
                            microphone_test_peak.swap(0.0_f32.to_bits(), Ordering::AcqRel),
                        );
                        let _ = microphone_test_events
                            .send(RuntimeMicrophoneTestEvent::Finished { peak_level });
                    }
                    Err(error) => {
                        let _ =
                            microphone_test_events.send(RuntimeMicrophoneTestEvent::Error(error));
                    }
                }
            }
            ControllerCommand::ConfirmPending => {
                let pending = get_pending_action(State::new(state.as_ref()))
                    .await
                    .ok()
                    .flatten();
                let transcription = pending
                    .as_ref()
                    .map(|pending| pending.transcription.clone())
                    .unwrap_or_default();
                match confirm_pending_action(State::new(state.as_ref())).await {
                    Ok(result) if result.success => {
                        let _ = events.send(RuntimeEvent::Success {
                            transcription,
                            summary: result.message,
                        });
                    }
                    Ok(result) => {
                        let _ = events.send(RuntimeEvent::Error(result.message));
                    }
                    Err(error) => {
                        let _ = events.send(RuntimeEvent::Error(error));
                    }
                }
            }
            ControllerCommand::CancelPending => {
                match cancel_pending_action(State::new(state.as_ref())).await {
                    Ok(_) => {
                        let _ = events.send(RuntimeEvent::Idle);
                    }
                    Err(error) => {
                        let _ = events.send(RuntimeEvent::Error(error));
                    }
                }
            }
            ControllerCommand::Settings(command) => {
                let state = state.clone();
                let configuration_events = configuration_events.clone();
                let configuration_snapshot = configuration_snapshot.clone();
                let settings_updates = settings_updates.clone();
                tokio::spawn(async move {
                    let _settings_guard = settings_updates.lock().await;
                    run_settings_command(
                        state,
                        command,
                        configuration_events,
                        configuration_snapshot,
                    )
                    .await;
                });
            }
            ControllerCommand::RefreshDashboard => {
                let state = state.clone();
                let dashboard_events = dashboard_events.clone();
                let dashboard_snapshot = dashboard_snapshot.clone();
                let dashboard_refreshes = dashboard_refreshes.clone();
                tokio::spawn(async move {
                    let _refresh_guard = dashboard_refreshes.lock().await;
                    refresh_dashboard_snapshot(state, dashboard_events, dashboard_snapshot).await;
                });
            }
            ControllerCommand::DashboardMutation(mutation) => {
                let state = state.clone();
                let dashboard_events = dashboard_events.clone();
                let dashboard_mutation_events = dashboard_mutation_events.clone();
                let dashboard_snapshot = dashboard_snapshot.clone();
                let dashboard_refreshes = dashboard_refreshes.clone();
                let settings_updates = settings_updates.clone();
                tokio::spawn(async move {
                    let _dashboard_guard = dashboard_refreshes.lock().await;
                    if mutation.modifies_config() {
                        let _settings_guard = settings_updates.lock().await;
                        run_dashboard_mutation(
                            state,
                            mutation,
                            dashboard_events,
                            dashboard_mutation_events,
                            dashboard_snapshot,
                        )
                        .await;
                    } else {
                        run_dashboard_mutation(
                            state,
                            mutation,
                            dashboard_events,
                            dashboard_mutation_events,
                            dashboard_snapshot,
                        )
                        .await;
                    }
                });
            }
            _ => {}
        }
    }
}

async fn run_settings_command(
    state: Arc<AppState>,
    command: RuntimeSettingsCommand,
    events: Sender<RuntimeConfigurationEvent>,
    snapshot_cache: Arc<Mutex<Option<RuntimeConfigurationSnapshot>>>,
) {
    let operation = command.operation();
    let _ = events.send(RuntimeConfigurationEvent::OperationStarted(operation));

    let result = apply_settings_command(state.as_ref(), command).await;

    let snapshot = load_configuration_snapshot(state.as_ref()).await;
    if let Ok(mut cached) = snapshot_cache.lock() {
        *cached = Some(snapshot.clone());
    }
    let _ = events.send(RuntimeConfigurationEvent::Snapshot(Box::new(snapshot)));

    match result {
        Ok(()) => {
            let _ = events.send(RuntimeConfigurationEvent::OperationFinished(operation));
        }
        Err(message) => {
            let _ = events.send(RuntimeConfigurationEvent::OperationFailed { operation, message });
        }
    }
}

async fn apply_settings_command(
    state: &AppState,
    command: RuntimeSettingsCommand,
) -> Result<(), String> {
    match command {
        RuntimeSettingsCommand::Refresh => Ok(()),
        RuntimeSettingsCommand::SelectModel(model) => {
            set_transcription_model(State::new(state), model).await?;
            if state.transcription.runtime_status().model_downloaded {
                state.transcription.preload_selected_model().await?;
            }
            Ok(())
        }
        RuntimeSettingsCommand::DownloadModel(model) => {
            let selected = state.transcription.settings().model == model;
            download_local_model(State::new(state), model).await?;
            if selected {
                state.transcription.preload_selected_model().await?;
            }
            Ok(())
        }
        RuntimeSettingsCommand::SelectMicrophone(device) => {
            set_audio_device(State::new(state), device).await?;
            Ok(())
        }
        RuntimeSettingsCommand::MarkOnboardingComplete => {
            let mut config = get_config(State::new(state)).await?;
            config.ui.onboarding_completed = true;
            set_config(State::new(state), config).await?;
            Ok(())
        }
        RuntimeSettingsCommand::SetTriggerHotkey(hotkey) => {
            set_trigger_hotkey(State::new(state), hotkey).await?;
            Ok(())
        }
        RuntimeSettingsCommand::SetAssistantHotkey(hotkey) => {
            set_assistant_hotkey(State::new(state), hotkey).await?;
            Ok(())
        }
        RuntimeSettingsCommand::SetLanguagePreferences {
            source_language,
            target_language,
        } => {
            set_language_preferences(State::new(state), source_language, target_language).await?;
            Ok(())
        }
        RuntimeSettingsCommand::SetVibeCodingConfig(config) => {
            set_vibe_coding_config(State::new(state), config).await?;
            Ok(())
        }
        RuntimeSettingsCommand::SetUseGpu(enabled) => {
            let mut config = get_config(State::new(state)).await?;
            config.use_gpu = enabled;
            set_config(State::new(state), config).await?;
            if state.transcription.runtime_status().model_downloaded {
                state.transcription.preload_selected_model().await?;
            }
            Ok(())
        }
        RuntimeSettingsCommand::SetAutoStart(enabled) => {
            let mut config = get_config(State::new(state)).await?;
            config.auto_start = enabled;
            set_config(State::new(state), config).await?;
            Ok(())
        }
        RuntimeSettingsCommand::InstallCommandTemplate(template_id) => {
            install_command_template_by_id(&template_id).await
        }
    }
}

async fn install_command_template_by_id(template_id: &str) -> Result<(), String> {
    install_command_template(template_id.to_string()).await?;
    Ok(())
}

async fn load_configuration_snapshot(state: &AppState) -> RuntimeConfigurationSnapshot {
    let config = state.config.lock().await.clone();
    let transcription_settings = state.transcription.settings();
    let transcription_status = state.transcription.runtime_status();

    let audio_devices = tokio::task::spawn_blocking(AudioState::get_devices).await;
    let (audio_devices, audio_devices_error) = match audio_devices {
        Ok(Ok(devices)) => (devices, None),
        Ok(Err(error)) => (Vec::new(), Some(error)),
        Err(error) => (
            Vec::new(),
            Some(format!("Failed to enumerate audio devices: {error}")),
        ),
    };

    let model_state = state.transcription.clone();
    let local_models = tokio::task::spawn_blocking(move || model_state.list_models()).await;
    let local_models = match local_models {
        Ok(Ok(models)) => models,
        Ok(Err(_)) | Err(_) => Vec::new(),
    };

    let command_templates = get_command_templates().await.unwrap_or_default();

    RuntimeConfigurationSnapshot {
        config,
        audio_devices,
        local_models,
        transcription_settings,
        transcription_status,
        command_templates,
        errors: RuntimeConfigurationErrors {
            audio_devices: audio_devices_error,
        },
    }
}

async fn run_dashboard_mutation(
    state: Arc<AppState>,
    mutation: RuntimeDashboardMutation,
    dashboard_events: Sender<RuntimeDashboardEvent>,
    mutation_events: Sender<RuntimeDashboardMutationEvent>,
    snapshot_cache: Arc<Mutex<Option<RuntimeDashboardSnapshot>>>,
) {
    let _ = mutation_events.send(RuntimeDashboardMutationEvent::Started);

    let result = apply_dashboard_mutation(state.as_ref(), mutation).await;
    let snapshot = load_dashboard_snapshot(state.as_ref()).await;
    if let Ok(mut cached) = snapshot_cache.lock() {
        *cached = Some(snapshot.clone());
    }
    let _ = dashboard_events.send(RuntimeDashboardEvent::Snapshot(Box::new(snapshot)));

    match result {
        Ok(message) => {
            let _ = mutation_events.send(RuntimeDashboardMutationEvent::Succeeded { message });
        }
        Err(message) => {
            let _ = mutation_events.send(RuntimeDashboardMutationEvent::Failed { message });
        }
    }
}

async fn apply_dashboard_mutation(
    state: &AppState,
    mutation: RuntimeDashboardMutation,
) -> Result<String, String> {
    match mutation {
        RuntimeDashboardMutation::ClearConversation => {
            clear_conversation(State::new(state)).await?;
            Ok("Conversation cleared".to_string())
        }
        RuntimeDashboardMutation::NewConversationSession => {
            let session_id = new_conversation_session(State::new(state)).await?;
            Ok(format!("Started conversation session {session_id}"))
        }
        RuntimeDashboardMutation::SetClipboard(content) => {
            set_clipboard(State::new(state), content).await?;
            Ok("Clipboard updated".to_string())
        }
        RuntimeDashboardMutation::SetIntegrationEnabled { name, enabled } => {
            let changed = set_integration_enabled(State::new(state), name.clone(), enabled).await?;
            if !changed {
                return Err(format!("Unknown integration: {name}"));
            }
            let state = if enabled { "enabled" } else { "disabled" };
            Ok(format!("{name} {state}"))
        }
        RuntimeDashboardMutation::SetCustomCommandEnabled { id, enabled } => {
            set_custom_command_enabled(id, enabled).await?;
            Ok(if enabled {
                "Command enabled".to_string()
            } else {
                "Command disabled".to_string()
            })
        }
        RuntimeDashboardMutation::ImportCustomCommands(commands) => {
            let count = commands.len();
            for command in commands {
                save_custom_command(command).await?;
            }
            Ok(format!(
                "Imported {count} custom command{}",
                if count == 1 { "" } else { "s" }
            ))
        }
        RuntimeDashboardMutation::SaveCustomCommand(command) => {
            let name = command.name.clone();
            save_custom_command(command).await?;
            Ok(format!("Saved command '{name}'"))
        }
        RuntimeDashboardMutation::DeleteCustomCommand(id) => {
            delete_custom_command(id).await?;
            Ok("Command deleted".to_string())
        }
        RuntimeDashboardMutation::AddDictionaryWord {
            word,
            is_auto_learned,
        } => {
            let entry = add_dictionary_word(word, is_auto_learned).await?;
            Ok(format!("Added '{}' to dictionary", entry.word))
        }
        RuntimeDashboardMutation::UpdateDictionaryWord { id, word, phonetic } => {
            update_dictionary_word(id, word.clone(), phonetic).await?;
            Ok(format!("Updated dictionary entry '{word}'"))
        }
        RuntimeDashboardMutation::DeleteDictionaryWord(id) => {
            delete_dictionary_word(id).await?;
            Ok("Dictionary entry deleted".to_string())
        }
        RuntimeDashboardMutation::CreateSnippet { trigger, expansion } => {
            let snippet = create_snippet(trigger, expansion).await?;
            Ok(format!("Created snippet '{}'", snippet.trigger))
        }
        RuntimeDashboardMutation::UpdateSnippet {
            id,
            trigger,
            expansion,
        } => {
            update_snippet(id, trigger.clone(), expansion).await?;
            Ok(format!("Updated snippet '{trigger}'"))
        }
        RuntimeDashboardMutation::DeleteSnippet(id) => {
            delete_snippet(id).await?;
            Ok("Snippet deleted".to_string())
        }
        RuntimeDashboardMutation::SetDictationStyle { context, style } => {
            let mut config = get_config(State::new(state)).await?;
            match context {
                RuntimeDictationStyleContext::Personal => config.dictation_style.personal = style,
                RuntimeDictationStyleContext::Work => config.dictation_style.work = style,
                RuntimeDictationStyleContext::Email => config.dictation_style.email = style,
                RuntimeDictationStyleContext::Other => config.dictation_style.other = style,
            }
            set_config(State::new(state), config).await?;
            Ok("Dictation style updated".to_string())
        }
    }
}

async fn refresh_dashboard_snapshot(
    state: Arc<AppState>,
    events: Sender<RuntimeDashboardEvent>,
    snapshot_cache: Arc<Mutex<Option<RuntimeDashboardSnapshot>>>,
) {
    let _ = events.send(RuntimeDashboardEvent::RefreshStarted);
    let snapshot = load_dashboard_snapshot(state.as_ref()).await;
    if let Ok(mut cached) = snapshot_cache.lock() {
        *cached = Some(snapshot.clone());
    }
    let _ = events.send(RuntimeDashboardEvent::Snapshot(Box::new(snapshot)));
}

async fn load_dashboard_snapshot(state: &AppState) -> RuntimeDashboardSnapshot {
    let (
        history,
        conversation,
        clipboard,
        clipboard_history,
        integrations,
        custom_commands,
        command_templates,
        dictionary_words,
        snippets,
        config,
    ) = tokio::join!(
        get_history(State::new(state)),
        get_conversation(State::new(state)),
        get_clipboard(State::new(state)),
        get_clipboard_history(State::new(state), Some(DASHBOARD_CLIPBOARD_HISTORY_LIMIT)),
        get_integrations(State::new(state)),
        get_custom_commands(),
        get_command_templates(),
        get_dictionary_words(),
        get_snippets(),
        get_config(State::new(state)),
    );

    let history = dashboard_value(history);
    let conversation = dashboard_value(conversation);
    let clipboard = dashboard_value(clipboard);
    let clipboard_history = dashboard_value(clipboard_history);
    let integrations = dashboard_value(integrations);
    let custom_commands = dashboard_value(custom_commands);
    let command_templates = dashboard_value(command_templates);
    let dictionary_words = dashboard_value(dictionary_words);
    let snippets = dashboard_value(snippets);
    let config = dashboard_value(config);

    RuntimeDashboardSnapshot {
        history,
        conversation,
        clipboard,
        clipboard_history,
        integrations,
        custom_commands,
        command_templates,
        dictionary_words,
        snippets,
        dictation_style: config.dictation_style,
    }
}

fn dashboard_value<T: Default>(result: Result<T, String>) -> T {
    result.unwrap_or_default()
}

async fn emit_processing_result(
    state: &AppState,
    result: Result<VoiceProcessingResult, String>,
    events: &Sender<RuntimeEvent>,
) {
    match result {
        Ok(result) if result.action.requires_confirmation => {
            let pending = get_pending_action(State::new(state)).await.ok().flatten();
            let summary = pending
                .map(|pending| pending.summary)
                .unwrap_or_else(|| "Review the pending native action".to_string());
            let _ = events.send(RuntimeEvent::ConfirmationRequired {
                transcription: result.transcription.text,
                summary,
            });
        }
        Ok(result) if result.action.action_type == "NoAction" => {
            let _ = events.send(RuntimeEvent::Idle);
        }
        Ok(result) if result.delivery_status.phase == DeliveryPhase::RecoverableFailure => {
            let summary = if result.delivery_status.summary.trim().is_empty() {
                "Text delivery needs attention".to_string()
            } else {
                result.delivery_status.summary
            };
            let _ = events.send(RuntimeEvent::Error(summary));
        }
        Ok(result) if !result.executed && result.action.action_type != "NoAction" => {
            let _ = events.send(RuntimeEvent::Error(format!(
                "{} was not executed",
                result.action.action_type
            )));
        }
        Ok(result) => {
            let summary = if result.transcription.text.trim().is_empty() {
                "No speech detected".to_string()
            } else if result.delivery_status.summary.trim().is_empty() {
                "Voice processing complete".to_string()
            } else {
                result.delivery_status.summary
            };
            let _ = events.send(RuntimeEvent::Success {
                transcription: result.transcription.text,
                summary,
            });
        }
        Err(error) => {
            let _ = events.send(RuntimeEvent::Error(error));
        }
    }
}

async fn run_audio_level_loop(
    state: Arc<AppState>,
    events: Sender<RuntimeEvent>,
    microphone_test_events: Sender<RuntimeMicrophoneTestEvent>,
    mut mode: watch::Receiver<AudioLevelMode>,
    microphone_test_peak: Arc<AtomicU32>,
) {
    let mut last_normal_level = 0.0_f32;
    loop {
        let current_mode = *mode.borrow();
        if current_mode != AudioLevelMode::Normal && last_normal_level != 0.0 {
            if events.send(RuntimeEvent::AudioLevel(0.0)).is_err() {
                break;
            }
            last_normal_level = 0.0;
        }

        if current_mode == AudioLevelMode::Idle {
            if mode.changed().await.is_err() {
                break;
            }
            continue;
        }

        tokio::select! {
            changed = mode.changed() => {
                if changed.is_err() {
                    break;
                }
                continue;
            }
            _ = tokio::time::sleep(AUDIO_LEVEL_INTERVAL) => {}
        }

        let current_mode = *mode.borrow();
        if current_mode == AudioLevelMode::Idle {
            continue;
        }
        let level = get_audio_level(State::new(state.as_ref()))
            .await
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        match current_mode {
            AudioLevelMode::Normal => {
                if events.send(RuntimeEvent::AudioLevel(level)).is_err() {
                    break;
                }
                last_normal_level = level;
            }
            AudioLevelMode::MicrophoneTest => {
                microphone_test_peak.fetch_max(level.to_bits(), Ordering::AcqRel);
                if microphone_test_events
                    .send(RuntimeMicrophoneTestEvent::Level(level))
                    .is_err()
                {
                    break;
                }
            }
            AudioLevelMode::Idle => {}
        }
    }
}

async fn run_status_loop(
    state: Arc<AppState>,
    events: Sender<RuntimeEvent>,
    microphone_test_active: Arc<AtomicBool>,
) {
    let mut interval = tokio::time::interval(STATUS_INTERVAL);
    loop {
        interval.tick().await;
        let status = match get_status(State::new(state.as_ref())).await {
            Ok(status) => status,
            Err(error) => {
                if events.send(RuntimeEvent::Error(error)).is_err() {
                    break;
                }
                continue;
            }
        };
        let snapshot = RuntimeStatusSnapshot {
            transcription: state.transcription.runtime_status(),
            audio: status.audio_status,
            is_listening: status.is_listening && !microphone_test_active.load(Ordering::Acquire),
            is_processing: status.is_processing,
            audio_device: status.audio_device,
        };
        if events
            .send(RuntimeEvent::Status(Box::new(snapshot)))
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(kind: &str, arch: &str, path: &str, url: &str, sha256: &str) -> ReleaseArtifact {
        ReleaseArtifact {
            kind: kind.to_string(),
            arch: arch.to_string(),
            path: path.to_string(),
            url: url.to_string(),
            sha256: sha256.to_string(),
        }
    }

    fn valid_artifacts(
        version: &str,
        mac_arch: &str,
    ) -> std::collections::BTreeMap<String, ReleaseArtifact> {
        let mut artifacts = std::collections::BTreeMap::new();
        let windows_path = format!("ListenOS-{version}-Setup-x86_64.exe");
        artifacts.insert(
            "windows".to_string(),
            artifact(
                "nsis-installer",
                "x86_64",
                &windows_path,
                &format!("https://updates.example.com/releases/v{version}/{windows_path}"),
                &"a".repeat(64),
            ),
        );
        let mac_path = format!("ListenOS-{version}-macos-{mac_arch}.dmg");
        artifacts.insert(
            "macos".to_string(),
            artifact(
                "dmg-installer",
                mac_arch,
                &mac_path,
                &format!("https://updates.example.com/releases/v{version}/{mac_path}"),
                &"b".repeat(64),
            ),
        );
        artifacts
    }

    #[test]
    fn update_manifest_selects_newer_hash_verified_platform_artifact() {
        let selected = select_release_artifact(
            ReleaseManifest {
                schema_version: UPDATE_MANIFEST_SCHEMA,
                version: "0.2.0".to_string(),
                artifacts: valid_artifacts("0.2.0", "universal"),
            },
            "0.1.21",
            "windows",
            "x86_64",
        )
        .expect("valid manifest")
        .expect("newer release");

        assert_eq!(selected.0, Version::parse("0.2.0").unwrap());
        assert_eq!(selected.1.kind, "nsis-installer");
    }

    #[test]
    fn update_manifest_treats_same_or_older_version_as_current() {
        let manifest = ReleaseManifest {
            schema_version: UPDATE_MANIFEST_SCHEMA,
            version: "0.1.21".to_string(),
            artifacts: valid_artifacts("0.1.21", "universal"),
        };
        assert!(
            select_release_artifact(manifest, "0.1.21", "windows", "x86_64")
                .expect("valid version")
                .is_none()
        );
    }

    #[test]
    fn update_manifest_rejects_unverified_or_insecure_artifacts() {
        for invalid_windows in [
            artifact(
                "nsis-installer",
                "x86_64",
                "ListenOS-0.2.0-Setup-x86_64.exe",
                "http://updates.example.com/releases/v0.2.0/ListenOS-0.2.0-Setup-x86_64.exe",
                &"a".repeat(64),
            ),
            artifact(
                "nsis-installer",
                "x86_64",
                "ListenOS-0.2.0-Setup-x86_64.exe",
                "https://updates.example.com/releases/v0.2.0/ListenOS-0.2.0-Setup-x86_64.exe",
                "not-a-hash",
            ),
        ] {
            let mut artifacts = valid_artifacts("0.2.0", "universal");
            artifacts.insert("windows".to_string(), invalid_windows);
            assert!(
                select_release_artifact(
                    ReleaseManifest {
                        schema_version: UPDATE_MANIFEST_SCHEMA,
                        version: "0.2.0".to_string(),
                        artifacts,
                    },
                    "0.1.21",
                    "windows",
                    "x86_64",
                )
                .is_err()
            );
        }
    }

    #[test]
    fn update_manifest_requires_packaged_artifact_kind_and_architecture() {
        assert!(
            select_release_artifact(
                ReleaseManifest {
                    schema_version: UPDATE_MANIFEST_SCHEMA,
                    version: "0.2.0".to_string(),
                    artifacts: valid_artifacts("0.2.0", "universal"),
                },
                "0.1.21",
                "macos",
                "aarch64",
            )
            .expect("valid macOS manifest")
            .is_some()
        );
    }

    #[test]
    fn generated_update_manifest_fixture_matches_runtime_contract() {
        let Ok(path) = std::env::var("LISTENOS_TEST_UPDATE_MANIFEST") else {
            return;
        };
        let bytes = std::fs::read(path).expect("read generated native-update.json fixture");
        let manifest: ReleaseManifest =
            serde_json::from_slice(&bytes).expect("generated manifest must deserialize");
        let windows = select_release_artifact(manifest.clone(), "0.0.0", "windows", "x86_64")
            .expect("generated manifest must satisfy runtime validation")
            .expect("generated manifest version must be newer than fixture current version");
        assert_eq!(windows.1.kind, "nsis-installer");

        assert_eq!(
            manifest
                .artifacts
                .get("macos")
                .expect("generated manifest must contain macOS artifact")
                .arch,
            "universal"
        );
        for runtime_arch in ["aarch64", "x86_64"] {
            let macos = select_release_artifact(manifest.clone(), "0.0.0", "macos", runtime_arch)
                .expect("generated macOS artifact must satisfy runtime validation")
                .expect("generated manifest version must be newer than fixture current version");
            assert_eq!(macos.1.kind, "dmg-installer");
            assert_eq!(macos.1.arch, "universal");
        }
    }
}
