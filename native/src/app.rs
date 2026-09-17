use crate::components::button::{ListenOsButton, ListenOsButtonVariant};
use crate::components::status_presentation::{MicrophoneStatus, ModelStatus};
use crate::platform_shell::{AutoStartState, ShellEvent, ShellServices};
use crate::runtime::{
    RuntimeConfigurationEvent, RuntimeConfigurationOperation, RuntimeConfigurationSnapshot,
    RuntimeController, RuntimeDashboardEvent, RuntimeDashboardMutationEvent,
    RuntimeDashboardSnapshot, RuntimeDictationStyleContext, RuntimeEvent,
    RuntimeMicrophoneTestEvent, RuntimeUpdateEvent,
};
use crate::state::{DashboardSection, OverlayState, RuntimeUiState};
use crate::theme::{DesignTokens, transparent};
use crate::views::dashboard::{
    ActivityGroup, ActivityRow, ClipboardAction, ClipboardActionProps, ClipboardContentKind,
    ClipboardHistoryItem, ClipboardProps, ClipboardQuickAction, ClipboardTab, CollectionTab,
    CommandActionEditorInputs, CommandCardProps, CommandEditorInputs, CommandsAction,
    CommandsActionProps, CommandsProps, CommandsTab, ConversationAction, ConversationActionProps,
    ConversationProps, DashboardAction, DashboardActionProps, DashboardProps, DashboardStats,
    DictionaryAction, DictionaryActionProps, DictionaryEditorInputs, DictionaryEntryProps,
    DictionaryProps, FeatureTipProps, GreetingProps, IntegrationActionProps, IntegrationCardProps,
    IntegrationsAction, IntegrationsActionProps, IntegrationsProps, SnippetEditorInputs,
    SnippetEntryProps, SnippetsAction, SnippetsActionProps, SnippetsProps, StyleAction,
    StyleActionProps, StyleContext, StyleProps, StyleTone, clipboard_view_with_actions,
    commands_view_with_actions, conversation_view_with_actions, dashboard_view_with_actions,
    dictionary_view_with_actions, integrations_view_with_actions, snippets_view_with_actions,
    style_view_with_actions,
};
use crate::views::onboarding::{
    OnboardingAction, OnboardingCommandChoice, OnboardingMicrophoneChoice, OnboardingModelChoice,
    OnboardingView, OnboardingViewProps,
};
use crate::views::onboarding_support::{OnboardingStep, OnboardingViewState};
use crate::views::settings::{
    SettingsAction, SettingsMicrophoneChoice, SettingsModelChoice, SettingsView, SettingsViewProps,
    ShortcutKind,
};
use crate::views::settings_support::{SettingsLanguageKind, SettingsViewState};
use gpui::{prelude::*, *};
use gpui_base::input::{InputEvent, InputState, TextareaState};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use voice_os_lib::{
    ActionStep, ClipboardContentType, CustomCommand, DictationStyle, Role,
    TranscriptionComputeBackend, TranscriptionRuntimePhase, TranscriptionRuntimeStatus,
};

const MICROPHONE_SIGNAL_THRESHOLD: f32 = 0.015;

#[derive(Debug, Clone)]
enum NativeUiAction {
    Onboarding(OnboardingAction),
    Settings(SettingsAction),
    Dashboard(DashboardAction),
    Conversation(ConversationAction),
    Commands(CommandsAction),
    Clipboard(ClipboardAction),
    Integrations(IntegrationsAction),
    Dictionary(DictionaryAction),
    Snippets(SnippetsAction),
    Style(StyleAction),
}

#[derive(Debug, Clone)]
enum PendingModelSetup {
    Downloading(String),
    Selecting(String),
}

#[derive(Debug, Clone)]
enum DictionaryEditorTarget {
    New,
    Existing(String),
}

#[derive(Debug, Clone)]
enum SnippetEditorTarget {
    New,
    Existing(String),
}

#[derive(Debug, Clone)]
enum PendingDestructiveDelete {
    Command(String),
    DictionaryWord(String),
    Snippet(String),
}

#[derive(Clone)]
struct CommandActionInputState {
    id: String,
    description: Option<String>,
    action_type: Entity<InputState>,
    payload: Entity<InputState>,
    delay_ms: Entity<InputState>,
}

pub struct ListenOsApp {
    tokens: DesignTokens,
    section: DashboardSection,
    overlay: OverlayState,
    runtime_ui: RuntimeUiState,
    configuration_loaded: bool,
    onboarding_complete: bool,
    settings_open: bool,
    ui_notice: Option<String>,
    configuration_snapshot: Option<RuntimeConfigurationSnapshot>,
    dashboard_snapshot: Option<RuntimeDashboardSnapshot>,
    dashboard_loading: bool,
    commands_tab: CommandsTab,
    clipboard_tab: ClipboardTab,
    clipboard_input: Option<Entity<TextareaState>>,
    clipboard_input_subscription: Option<Subscription>,
    clipboard_editor_source: String,
    clipboard_editor_value: String,
    dictionary_tab: CollectionTab,
    snippets_tab: CollectionTab,
    dictionary_search: String,
    snippets_search: String,
    dictionary_search_input: Option<Entity<InputState>>,
    dictionary_search_subscription: Option<Subscription>,
    dictionary_search_pending_value: Option<String>,
    snippets_search_input: Option<Entity<InputState>>,
    snippets_search_subscription: Option<Subscription>,
    snippets_search_pending_value: Option<String>,
    dictionary_editor: Option<DictionaryEditorTarget>,
    dictionary_word_input: Option<Entity<InputState>>,
    dictionary_phonetic_input: Option<Entity<InputState>>,
    dictionary_editor_pending_values: Option<(String, String)>,
    snippets_editor: Option<SnippetEditorTarget>,
    snippets_trigger_input: Option<Entity<InputState>>,
    snippets_expansion_input: Option<Entity<TextareaState>>,
    snippets_editor_pending_values: Option<(String, String)>,
    pending_destructive_delete: Option<PendingDestructiveDelete>,
    command_editor: Option<CustomCommand>,
    command_name_input: Option<Entity<InputState>>,
    command_trigger_input: Option<Entity<InputState>>,
    command_description_input: Option<Entity<TextareaState>>,
    command_action_inputs: Vec<CommandActionInputState>,
    command_pending_action_type: Option<String>,
    command_editor_pending_values: Option<(String, String, String, Vec<ActionStep>)>,
    style_context: StyleContext,
    expanded_integrations: HashSet<String>,
    onboarding: Entity<OnboardingView>,
    settings: Entity<SettingsView>,
    ui_action_tx: mpsc::Sender<NativeUiAction>,
    pending_model_setup: Option<PendingModelSetup>,
    model_operation_in_progress: Option<RuntimeConfigurationOperation>,
    pending_template_installs: usize,
    template_install_failed: bool,
    complete_after_templates: bool,
    update_status: String,
    update_checking: bool,
    dashboard_window: Option<WindowHandle<ListenOsApp>>,
    status_overlay_window: Option<WindowHandle<crate::overlay::StatusOverlayView>>,
    status_overlay_opening: bool,
    controls_overlay_window: Option<WindowHandle<crate::overlay::OverlayControlsView>>,
    controls_overlay_opening: bool,
    shell: ShellServices,
    runtime: RuntimeController,
    _runtime_bridge: Task<()>,
}

impl ListenOsApp {
    pub fn new(mut runtime: RuntimeController, cx: &mut Context<Self>) -> Self {
        let shortcut_initialization_error = runtime.initialize_shortcuts().err();
        let events = runtime.events();
        let update_events = runtime.update_events();
        let configuration_events = runtime.configuration_events();
        let microphone_test_events = runtime.microphone_test_events();
        let dashboard_events = runtime.dashboard_events();
        let dashboard_mutation_events = runtime.dashboard_mutation_events();
        let (ui_action_tx, ui_action_rx) = mpsc::channel::<NativeUiAction>();
        let ui_actions = Arc::new(Mutex::new(ui_action_rx));
        let shell = ShellServices::new();
        let shell_events = shell.events();

        let onboarding_tx = ui_action_tx.clone();
        let onboarding = cx.new(|_| {
            OnboardingView::new(
                OnboardingViewState::default(),
                OnboardingViewProps {
                    on_action: Some(Arc::new(move |action| {
                        let _ = onboarding_tx.send(NativeUiAction::Onboarding(action));
                    })),
                    ..OnboardingViewProps::default()
                },
            )
        });
        let settings_tx = ui_action_tx.clone();
        let settings = cx.new(|_| {
            SettingsView::new(
                SettingsViewState::default(),
                SettingsViewProps {
                    on_action: Some(Arc::new(move |action| {
                        let _ = settings_tx.send(NativeUiAction::Settings(action));
                    })),
                    ..SettingsViewProps::default()
                },
            )
        });

        let runtime_bridge = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;

                let pending_runtime = match events.lock() {
                    Ok(receiver) => receiver.try_iter().collect::<Vec<_>>(),
                    Err(_) => return,
                };
                let pending_updates = match update_events.lock() {
                    Ok(receiver) => receiver.try_iter().collect::<Vec<_>>(),
                    Err(_) => return,
                };
                let pending_configuration = match configuration_events.lock() {
                    Ok(receiver) => receiver.try_iter().collect::<Vec<_>>(),
                    Err(_) => return,
                };
                let pending_microphone = match microphone_test_events.lock() {
                    Ok(receiver) => receiver.try_iter().collect::<Vec<_>>(),
                    Err(_) => return,
                };
                let pending_dashboard = match dashboard_events.lock() {
                    Ok(receiver) => receiver.try_iter().collect::<Vec<_>>(),
                    Err(_) => return,
                };
                let pending_dashboard_mutations = match dashboard_mutation_events.lock() {
                    Ok(receiver) => receiver.try_iter().collect::<Vec<_>>(),
                    Err(_) => return,
                };
                let pending_ui = match ui_actions.lock() {
                    Ok(receiver) => receiver.try_iter().collect::<Vec<_>>(),
                    Err(_) => return,
                };
                let pending_shell = match shell_events.lock() {
                    Ok(receiver) => receiver.try_iter().collect::<Vec<_>>(),
                    Err(_) => return,
                };
                if pending_runtime.is_empty()
                    && pending_updates.is_empty()
                    && pending_configuration.is_empty()
                    && pending_microphone.is_empty()
                    && pending_dashboard.is_empty()
                    && pending_dashboard_mutations.is_empty()
                    && pending_ui.is_empty()
                    && pending_shell.is_empty()
                {
                    continue;
                }

                if this
                    .update(cx, |this, cx| {
                        for event in pending_runtime {
                            let auto_idle = match &event {
                                RuntimeEvent::Success { .. } => {
                                    Some((OverlayState::Success, Duration::from_millis(900)))
                                }
                                RuntimeEvent::Error(_) => {
                                    Some((OverlayState::Error, Duration::from_millis(1500)))
                                }
                                _ => None,
                            };
                            if let RuntimeEvent::Status(status) = &event {
                                this.apply_live_transcription_status(&status.transcription, cx);
                            }
                            this.apply_runtime_event(event, cx);
                            if let Some((expected, delay)) = auto_idle {
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor().timer(delay).await;
                                    let _ = this.update(cx, |this, cx| {
                                        if this.overlay == expected {
                                            this.overlay = OverlayState::Idle;
                                            this.runtime_ui.overlay_detail =
                                                this.hold_to_talk_shortcut_label();
                                            this.reconcile_overlays(cx);
                                            cx.notify();
                                        }
                                    });
                                })
                                .detach();
                            }
                        }

                        for event in pending_updates {
                            this.apply_update_event(event, cx);
                        }

                        for event in pending_configuration {
                            this.apply_configuration_event(event, cx);
                        }
                        for event in pending_microphone {
                            this.apply_microphone_test_event(event, cx);
                        }
                        for event in pending_dashboard {
                            match event {
                                RuntimeDashboardEvent::RefreshStarted => {
                                    this.dashboard_loading = true;
                                }
                                RuntimeDashboardEvent::Snapshot(snapshot) => {
                                    this.dashboard_loading = false;
                                    this.dashboard_snapshot = Some(*snapshot);
                                }
                            }
                        }
                        for event in pending_dashboard_mutations {
                            match event {
                                RuntimeDashboardMutationEvent::Started => {}
                                RuntimeDashboardMutationEvent::Succeeded { message } => {
                                    this.ui_notice = Some(message);
                                }
                                RuntimeDashboardMutationEvent::Failed { message } => {
                                    this.ui_notice = Some(message);
                                }
                            }
                        }
                        for action in pending_ui {
                            this.apply_ui_action(action, cx);
                        }
                        for event in pending_shell {
                            this.apply_shell_event(event, cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        });

        let mut app = Self {
            tokens: DesignTokens::default(),
            section: DashboardSection::Dashboard,
            overlay: OverlayState::Idle,
            runtime_ui: RuntimeUiState::default(),
            configuration_loaded: false,
            onboarding_complete: false,
            settings_open: false,
            ui_notice: None,
            configuration_snapshot: None,
            dashboard_snapshot: runtime.dashboard_snapshot(),
            dashboard_loading: true,
            commands_tab: CommandsTab::Commands,
            clipboard_tab: ClipboardTab::Current,
            clipboard_input: None,
            clipboard_input_subscription: None,
            clipboard_editor_source: String::new(),
            clipboard_editor_value: String::new(),
            dictionary_tab: CollectionTab::All,
            snippets_tab: CollectionTab::All,
            dictionary_search: String::new(),
            snippets_search: String::new(),
            dictionary_search_input: None,
            dictionary_search_subscription: None,
            dictionary_search_pending_value: None,
            snippets_search_input: None,
            snippets_search_subscription: None,
            snippets_search_pending_value: None,
            dictionary_editor: None,
            dictionary_word_input: None,
            dictionary_phonetic_input: None,
            dictionary_editor_pending_values: None,
            snippets_editor: None,
            snippets_trigger_input: None,
            snippets_expansion_input: None,
            snippets_editor_pending_values: None,
            pending_destructive_delete: None,
            command_editor: None,
            command_name_input: None,
            command_trigger_input: None,
            command_description_input: None,
            command_action_inputs: Vec::new(),
            command_pending_action_type: None,
            command_editor_pending_values: None,
            style_context: StyleContext::Personal,
            expanded_integrations: HashSet::new(),
            onboarding,
            settings,
            ui_action_tx,
            pending_model_setup: None,
            model_operation_in_progress: None,
            pending_template_installs: 0,
            template_install_failed: false,
            complete_after_templates: false,
            update_status: if cfg!(debug_assertions) {
                "Updates are disabled in development builds.".to_string()
            } else {
                "Checking for updates…".to_string()
            },
            update_checking: !cfg!(debug_assertions),
            dashboard_window: None,
            status_overlay_window: None,
            status_overlay_opening: false,
            controls_overlay_window: None,
            controls_overlay_opening: false,
            shell,
            runtime,
            _runtime_bridge: runtime_bridge,
        };

        if let Some(error) = shortcut_initialization_error {
            app.runtime_ui.shortcut_status = error;
        } else {
            app.runtime_ui.shortcut_status = "Shortcuts ready".to_string();
        }

        if let Some(snapshot) = app.runtime.configuration_snapshot() {
            app.apply_configuration_snapshot(snapshot, cx);
        }
        app
    }

    pub(crate) fn attach_dashboard_window(&mut self, window: WindowHandle<ListenOsApp>) {
        self.dashboard_window = Some(window);
    }

    pub(crate) fn tray_active(&self) -> bool {
        self.shell.tray_enabled().unwrap_or(false)
    }

    fn reconcile_overlays(&mut self, cx: &mut Context<Self>) {
        let should_be_open = matches!(
            self.overlay,
            OverlayState::Listening
                | OverlayState::Processing
                | OverlayState::Success
                | OverlayState::Error
        );

        if should_be_open && self.status_overlay_window.is_none() && !self.status_overlay_opening {
            self.status_overlay_opening = true;
            let app = cx.entity();
            cx.defer(move |cx| {
                let opened = crate::overlay::open_status_overlay(app.clone(), cx);
                let mut close_after_open = None;
                app.update(cx, |this, _| {
                    this.status_overlay_opening = false;
                    match opened {
                        Ok(window)
                            if matches!(
                                this.overlay,
                                OverlayState::Listening
                                    | OverlayState::Processing
                                    | OverlayState::Success
                                    | OverlayState::Error
                            ) =>
                        {
                            this.status_overlay_window = Some(window);
                        }
                        Ok(window) => close_after_open = Some(window),
                        Err(error) => {
                            this.ui_notice =
                                Some(format!("Could not open status overlay: {error}"));
                        }
                    }
                });
                if let Some(window_handle) = close_after_open {
                    let _ = window_handle.update(cx, |_, window, _| window.remove_window());
                }
            });
        } else if !should_be_open && let Some(window_handle) = self.status_overlay_window.take() {
            cx.defer(move |cx| {
                let _ = window_handle.update(cx, |_, window, _| window.remove_window());
            });
        }

        let controls_should_be_open = matches!(
            self.overlay,
            OverlayState::Handsfree | OverlayState::ConfirmationRequired
        );

        if controls_should_be_open
            && self.controls_overlay_window.is_none()
            && !self.controls_overlay_opening
        {
            self.controls_overlay_opening = true;
            let app = cx.entity();
            cx.defer(move |cx| {
                let opened = crate::overlay::open_controls_overlay(app.clone(), cx);
                let mut close_after_open = None;
                app.update(cx, |this, _| {
                    this.controls_overlay_opening = false;
                    match opened {
                        Ok(window)
                            if matches!(
                                this.overlay,
                                OverlayState::Handsfree | OverlayState::ConfirmationRequired
                            ) =>
                        {
                            this.controls_overlay_window = Some(window);
                        }
                        Ok(window) => close_after_open = Some(window),
                        Err(error) => {
                            this.ui_notice =
                                Some(format!("Could not open controls overlay: {error}"));
                        }
                    }
                });
                if let Some(window_handle) = close_after_open {
                    let _ = window_handle.update(cx, |_, window, _| window.remove_window());
                }
            });
        } else if !controls_should_be_open
            && let Some(window_handle) = self.controls_overlay_window.take()
        {
            cx.defer(move |cx| {
                let _ = window_handle.update(cx, |_, window, _| window.remove_window());
            });
        }
    }

    fn apply_shell_event(&mut self, event: ShellEvent, cx: &mut Context<Self>) {
        match event {
            ShellEvent::OpenDashboard => {
                let Some(window_handle) = self.dashboard_window else {
                    return;
                };
                cx.defer(move |cx| {
                    let _ = window_handle.update(cx, |_, window, cx| {
                        crate::platform_shell::show_dashboard_window(window);
                        cx.activate(true);
                    });
                });
            }
            ShellEvent::Quit => cx.quit(),
        }
    }

    fn apply_update_event(&mut self, event: RuntimeUpdateEvent, cx: &mut Context<Self>) {
        match event {
            RuntimeUpdateEvent::Checking => {
                self.update_checking = true;
                self.update_status = "Checking for updates…".to_string();
            }
            RuntimeUpdateEvent::UpToDate => {
                self.update_checking = false;
                self.update_status = "You're on the latest version.".to_string();
            }
            RuntimeUpdateEvent::Downloading { version } => {
                self.update_checking = true;
                self.update_status = format!("Downloading ListenOS {version}…");
            }
            RuntimeUpdateEvent::InstallerLaunched { version } => {
                self.update_checking = false;
                self.update_status = format!("Launching ListenOS {version} installer…");
                cx.quit();
                return;
            }
            RuntimeUpdateEvent::DisabledInDevelopment => {
                self.update_checking = false;
                self.update_status = "Updates are disabled in development builds.".to_string();
            }
            RuntimeUpdateEvent::Failed { message } => {
                self.update_checking = false;
                self.update_status = format!("Update check failed: {message}");
            }
        }
        if let Some(snapshot) = self.configuration_snapshot.clone() {
            self.apply_configuration_snapshot(snapshot, cx);
        }
    }

    pub(crate) fn overlay_snapshot(&self) -> (OverlayState, f32) {
        (self.overlay, self.runtime_ui.audio_level)
    }

    pub(crate) fn overlay_controls_snapshot(&self) -> (OverlayState, String) {
        (self.overlay, self.runtime_ui.overlay_detail.clone())
    }

    pub(crate) fn overlay_confirm_pending(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.runtime.confirm_pending() {
            self.apply_runtime_event(RuntimeEvent::Error(error), cx);
        }
    }

    pub(crate) fn overlay_cancel_pending(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.runtime.cancel_pending() {
            self.apply_runtime_event(RuntimeEvent::Error(error), cx);
        }
    }

    pub(crate) fn overlay_cancel_capture(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.runtime.cancel_capture() {
            self.apply_runtime_event(RuntimeEvent::Error(error), cx);
        }
    }

    pub(crate) fn overlay_stop_handsfree(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.runtime.stop_capture(true) {
            self.apply_runtime_event(RuntimeEvent::Error(error), cx);
        }
    }

    fn apply_runtime_event(&mut self, event: RuntimeEvent, cx: &mut Context<Self>) {
        match event {
            RuntimeEvent::Status(status) => {
                self.runtime_ui.model = status.transcription.model.clone();
                self.runtime_ui.transcription_phase = format!("{:?}", status.transcription.phase);
                self.runtime_ui.audio_phase = format!("{:?}", status.audio.phase);
                self.runtime_ui.audio_device = status
                    .audio_device
                    .or(status.audio.device_name)
                    .unwrap_or_else(|| "Default microphone".to_string());
                if status.is_listening {
                    self.runtime_ui.shortcut_status = "Shortcut active".to_string();
                } else if status.is_processing {
                    self.runtime_ui.shortcut_status = "Processing".to_string();
                    self.runtime_ui.overlay_detail = match status.transcription.phase {
                        TranscriptionRuntimePhase::Transcribing => {
                            format!("Transcribing locally · {}", status.transcription.model)
                        }
                        _ => "Finishing local processing…".to_string(),
                    };
                } else if self.runtime_ui.shortcut_status == "Registering shortcuts" {
                    self.runtime_ui.shortcut_status = "Shortcuts ready".to_string();
                }
            }
            RuntimeEvent::AudioLevel(level) => {
                self.runtime_ui.audio_level = level;
            }
            RuntimeEvent::Idle => {
                self.overlay = OverlayState::Idle;
                self.runtime_ui.overlay_detail = self.hold_to_talk_shortcut_label();
            }
            RuntimeEvent::Listening { handsfree } => {
                self.overlay = if handsfree {
                    OverlayState::Handsfree
                } else {
                    OverlayState::Listening
                };
                self.runtime_ui.overlay_detail = if handsfree {
                    "Assistant shortcut active · press again to process".to_string()
                } else {
                    "Release shortcut to process".to_string()
                };
            }
            RuntimeEvent::Processing { handsfree } => {
                self.overlay = OverlayState::Processing;
                self.runtime_ui.overlay_detail = if handsfree {
                    "Running local hands-free dictation".to_string()
                } else {
                    "Running local transcription".to_string()
                };
            }
            RuntimeEvent::Success {
                transcription,
                summary,
            } => {
                self.overlay = OverlayState::Success;
                self.runtime_ui.overlay_detail = summary;
                if !transcription.trim().is_empty() {
                    self.runtime_ui.last_transcription = transcription;
                }
            }
            RuntimeEvent::ConfirmationRequired {
                transcription,
                summary,
            } => {
                self.overlay = OverlayState::ConfirmationRequired;
                self.runtime_ui.overlay_detail = summary;
                if !transcription.trim().is_empty() {
                    self.runtime_ui.last_transcription = transcription;
                }
            }
            RuntimeEvent::Error(message) => {
                self.overlay = OverlayState::Error;
                self.runtime_ui.overlay_detail = message;
            }
        }
        self.reconcile_overlays(cx);
    }

    fn shortcut_label(raw: &str) -> String {
        raw.split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" + ")
    }

    fn hold_to_talk_shortcut_label(&self) -> String {
        self.configuration_snapshot
            .as_ref()
            .map(|snapshot| Self::shortcut_label(&snapshot.config.trigger_hotkey))
            .unwrap_or_else(|| {
                if cfg!(target_os = "macos") {
                    "Ctrl + Space".to_string()
                } else {
                    "Meta + Ctrl + Space".to_string()
                }
            })
    }

    fn model_status_from_runtime(status: &TranscriptionRuntimeStatus) -> ModelStatus {
        match status.phase {
            TranscriptionRuntimePhase::Ready | TranscriptionRuntimePhase::Transcribing => {
                ModelStatus::Ready {
                    model: status.model.clone().into(),
                }
            }
            TranscriptionRuntimePhase::Loading => ModelStatus::Loading {
                model: status.model.clone().into(),
            },
            TranscriptionRuntimePhase::ModelMissing => ModelStatus::Missing {
                model: status.model.clone().into(),
            },
            TranscriptionRuntimePhase::Downloading => {
                let progress_percent = match (status.download_bytes, status.download_total_bytes) {
                    (Some(downloaded), Some(total)) if total > 0 => {
                        Some(((downloaded.saturating_mul(100) / total).min(100)) as u8)
                    }
                    _ => None,
                };
                ModelStatus::Downloading {
                    model: status.model.clone().into(),
                    progress_percent,
                    downloaded_bytes: status.download_bytes,
                    total_bytes: status.download_total_bytes,
                }
            }
            TranscriptionRuntimePhase::Error => ModelStatus::Error {
                model: Some(status.model.clone().into()),
                message: status
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "Local transcription failed".to_string())
                    .into(),
            },
        }
    }

    fn set_model_status(&mut self, model_status: ModelStatus, cx: &mut Context<Self>) {
        let mut onboarding_state = self.onboarding.read(cx).state().clone();
        onboarding_state.model_status = model_status.clone();
        self.onboarding
            .update(cx, |view, cx| view.set_state(onboarding_state, cx));

        let mut settings_state = self.settings.read(cx).state().clone();
        settings_state.model_status = model_status;
        self.settings
            .update(cx, |view, cx| view.set_state(settings_state, cx));
    }

    fn apply_live_transcription_status(
        &mut self,
        status: &TranscriptionRuntimeStatus,
        cx: &mut Context<Self>,
    ) {
        if let Some(snapshot) = self.configuration_snapshot.as_mut() {
            snapshot.transcription_status = status.clone();
        }

        // A model select operation deliberately shows "Loading" until its
        // preload completes. Do not let the periodic Ready snapshot erase that
        // feedback while the operation is still running.
        if self.model_operation_in_progress == Some(RuntimeConfigurationOperation::SelectModel)
            && matches!(status.phase, TranscriptionRuntimePhase::Ready)
        {
            return;
        }

        // Likewise, keep the last concrete download progress visible until the
        // download command publishes its final configuration snapshot.
        if self.model_operation_in_progress == Some(RuntimeConfigurationOperation::DownloadModel)
            && !matches!(
                status.phase,
                TranscriptionRuntimePhase::Downloading | TranscriptionRuntimePhase::Error
            )
        {
            return;
        }

        self.set_model_status(Self::model_status_from_runtime(status), cx);
    }

    fn model_status(snapshot: &RuntimeConfigurationSnapshot) -> ModelStatus {
        Self::model_status_from_runtime(&snapshot.transcription_status)
    }

    fn is_handsfree_device(name: &str) -> bool {
        let normalized = name.to_lowercase();
        normalized.contains("hands-free")
            || normalized.contains("hands free")
            || normalized.contains("ag audio")
            || normalized.contains("hfp")
            || normalized.contains("hsp")
    }

    fn preferred_microphone(snapshot: &RuntimeConfigurationSnapshot) -> Option<String> {
        snapshot
            .config
            .selected_audio_device
            .clone()
            .filter(|selected| {
                snapshot.audio_devices.iter().any(|device| {
                    device.name == *selected && !Self::is_handsfree_device(&device.name)
                })
            })
            .or_else(|| {
                snapshot
                    .audio_devices
                    .iter()
                    .find(|device| device.is_default && !Self::is_handsfree_device(&device.name))
                    .or_else(|| {
                        snapshot
                            .audio_devices
                            .iter()
                            .find(|device| !Self::is_handsfree_device(&device.name))
                    })
                    .map(|device| device.name.clone())
            })
    }

    fn microphone_status(
        snapshot: &RuntimeConfigurationSnapshot,
        selected: Option<&str>,
    ) -> MicrophoneStatus {
        if let Some(error) = snapshot.errors.audio_devices.clone() {
            return MicrophoneStatus::Unavailable {
                message: error.into(),
            };
        }
        let Some(name) = selected else {
            return MicrophoneStatus::Unavailable {
                message: "No supported microphone is available".into(),
            };
        };
        let is_default = snapshot
            .audio_devices
            .iter()
            .find(|device| device.name == name)
            .map(|device| device.is_default)
            .unwrap_or(false);
        MicrophoneStatus::Selected {
            name: name.to_string().into(),
            is_default,
        }
    }

    fn onboarding_callback(&self) -> Arc<dyn Fn(OnboardingAction) + Send + Sync + 'static> {
        let sender = self.ui_action_tx.clone();
        Arc::new(move |action| {
            let _ = sender.send(NativeUiAction::Onboarding(action));
        })
    }

    fn settings_callback(&self) -> Arc<dyn Fn(SettingsAction) + Send + Sync + 'static> {
        let sender = self.ui_action_tx.clone();
        Arc::new(move |action| {
            let _ = sender.send(NativeUiAction::Settings(action));
        })
    }

    fn apply_configuration_snapshot(
        &mut self,
        snapshot: RuntimeConfigurationSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.configuration_loaded = true;
        self.onboarding_complete = snapshot.config.ui.onboarding_completed;

        let selected_model = snapshot.transcription_settings.model.clone();
        let selected_microphone = Self::preferred_microphone(&snapshot);
        let model_status = Self::model_status(&snapshot);
        let microphone_status = Self::microphone_status(&snapshot, selected_microphone.as_deref());

        let onboarding_models = snapshot
            .local_models
            .iter()
            .map(|model| OnboardingModelChoice {
                id: model.id.clone().into(),
                label: model.label.clone().into(),
                detail: if model.downloaded {
                    "Downloaded on this device".into()
                } else {
                    "Downloads locally before first use".into()
                },
                downloaded: model.downloaded,
            })
            .collect::<Vec<_>>();
        let onboarding_microphones = snapshot
            .audio_devices
            .iter()
            .map(|device| OnboardingMicrophoneChoice {
                name: device.name.clone().into(),
                detail: device
                    .sample_rate
                    .map(|rate| format!("{} Hz", rate))
                    .unwrap_or_else(|| "Input device".to_string())
                    .into(),
                is_default: device.is_default,
                blocked: Self::is_handsfree_device(&device.name),
            })
            .collect::<Vec<_>>();
        let command_templates = snapshot
            .command_templates
            .iter()
            .map(|command| OnboardingCommandChoice {
                id: command.id.clone().into(),
                name: command.name.clone().into(),
                trigger_phrase: command.trigger_phrase.clone().into(),
            })
            .collect::<Vec<_>>();

        let mut onboarding_state = self.onboarding.read(cx).state().clone();
        if onboarding_state.selected_model.is_none() {
            onboarding_state.selected_model = Some(selected_model.clone().into());
        }
        if onboarding_state.selected_microphone.is_none() {
            onboarding_state.selected_microphone = selected_microphone.clone().map(Into::into);
        }
        onboarding_state.model_status = model_status.clone();
        if !matches!(
            onboarding_state.microphone_status,
            MicrophoneStatus::Testing { .. } | MicrophoneStatus::Ready { .. }
        ) {
            onboarding_state.microphone_status = microphone_status.clone();
        }
        let onboarding_props = OnboardingViewProps {
            hold_to_talk_shortcut: Self::shortcut_label(&snapshot.config.trigger_hotkey).into(),
            models: onboarding_models,
            microphones: onboarding_microphones,
            command_templates,
            on_action: Some(self.onboarding_callback()),
        };
        self.onboarding.update(cx, |view, cx| {
            view.set_state(onboarding_state, cx);
            view.set_props(onboarding_props, cx);
        });

        let settings_models = snapshot
            .local_models
            .iter()
            .map(|model| SettingsModelChoice {
                id: model.id.clone().into(),
                label: model.label.clone().into(),
                detail: if model.downloaded {
                    "Ready for local transcription".into()
                } else {
                    "Not downloaded".into()
                },
                downloaded: model.downloaded,
            })
            .collect::<Vec<_>>();
        let settings_microphones = snapshot
            .audio_devices
            .iter()
            .map(|device| SettingsMicrophoneChoice {
                name: device.name.clone().into(),
                detail: device
                    .sample_rate
                    .map(|rate| format!("{} Hz", rate))
                    .unwrap_or_else(|| "Input device".to_string())
                    .into(),
                is_default: device.is_default,
                blocked: Self::is_handsfree_device(&device.name),
            })
            .collect::<Vec<_>>();
        let mut settings_state = self.settings.read(cx).state().clone();
        settings_state.selected_model = Some(selected_model.into());
        settings_state.selected_microphone = selected_microphone.map(Into::into);
        settings_state.model_status = model_status;
        settings_state.microphone_status = microphone_status;
        let shell_capabilities = self.shell.capabilities();
        let start_on_login = if shell_capabilities.auto_start {
            self.shell
                .auto_start_status()
                .map(|state| state.requested())
                .unwrap_or(snapshot.config.auto_start)
        } else {
            snapshot.config.auto_start
        };
        let gpu_status: SharedString = match snapshot.transcription_status.active_backend {
            Some(TranscriptionComputeBackend::Gpu) => snapshot
                .transcription_status
                .accelerator
                .as_ref()
                .map(|accelerator| format!("Active · {accelerator}"))
                .unwrap_or_else(|| "GPU acceleration is active".to_string())
                .into(),
            Some(TranscriptionComputeBackend::Cpu)
                if snapshot.transcription_status.gpu_requested =>
            {
                snapshot
                    .transcription_status
                    .backend_fallback_reason
                    .clone()
                    .unwrap_or_else(|| "GPU requested; using CPU fallback".to_string())
                    .into()
            }
            Some(TranscriptionComputeBackend::Cpu) => {
                "GPU disabled · transcription is using CPU".into()
            }
            None if snapshot.config.use_gpu => snapshot
                .transcription_status
                .backend_fallback_reason
                .clone()
                .unwrap_or_else(|| "GPU preferred; backend is not loaded yet".to_string())
                .into(),
            None => "GPU disabled · transcription will use CPU".into(),
        };
        let settings_props = SettingsViewProps {
            app_version: env!("CARGO_PKG_VERSION").into(),
            hold_to_talk_shortcut: Self::shortcut_label(&snapshot.config.trigger_hotkey).into(),
            assistant_shortcut: Self::shortcut_label(&snapshot.config.assistant_hotkey).into(),
            source_language: snapshot
                .config
                .language_preferences
                .source_language
                .clone()
                .into(),
            target_language: snapshot
                .config
                .language_preferences
                .target_language
                .clone()
                .into(),
            update_status: self.update_status.clone().into(),
            update_checking: self.update_checking,
            start_on_login,
            auto_start_supported: shell_capabilities.auto_start,
            show_in_tray: self.shell.tray_enabled().unwrap_or(false),
            tray_supported: shell_capabilities.tray,
            use_gpu: snapshot.config.use_gpu,
            gpu_status,
            vibe_coding: snapshot.config.vibe_coding.clone(),
            models: settings_models,
            microphones: settings_microphones,
            on_action: Some(self.settings_callback()),
        };
        self.settings.update(cx, |view, cx| {
            view.set_state(settings_state, cx);
            view.set_props(settings_props, cx);
        });

        if self.overlay == OverlayState::Idle {
            self.runtime_ui.overlay_detail = Self::shortcut_label(&snapshot.config.trigger_hotkey);
        }

        self.configuration_snapshot = Some(snapshot);
    }

    fn advance_onboarding_to(&mut self, step: OnboardingStep, cx: &mut Context<Self>) {
        let mut state = self.onboarding.read(cx).state().clone();
        state.step = step;
        self.onboarding
            .update(cx, |view, cx| view.set_state(state, cx));
    }

    fn apply_configuration_event(
        &mut self,
        event: RuntimeConfigurationEvent,
        cx: &mut Context<Self>,
    ) {
        let mut event = event;
        self.runtime
            .reconcile_shortcut_configuration_event(&mut event);
        match event {
            RuntimeConfigurationEvent::Snapshot(snapshot) => {
                self.apply_configuration_snapshot(*snapshot, cx);
            }
            RuntimeConfigurationEvent::OperationStarted(operation) => match operation {
                RuntimeConfigurationOperation::DownloadModel => {
                    self.model_operation_in_progress = Some(operation);
                    let model = match self.pending_model_setup.as_ref() {
                        Some(PendingModelSetup::Downloading(model)) => model.clone().into(),
                        _ => self
                            .settings
                            .read(cx)
                            .state()
                            .selected_model
                            .clone()
                            .unwrap_or_else(|| "model".into()),
                    };
                    self.set_model_status(
                        ModelStatus::Downloading {
                            model,
                            progress_percent: None,
                            downloaded_bytes: Some(0),
                            total_bytes: None,
                        },
                        cx,
                    );
                }
                RuntimeConfigurationOperation::SelectModel => {
                    self.model_operation_in_progress = Some(operation);
                    let model = match self.pending_model_setup.as_ref() {
                        Some(PendingModelSetup::Selecting(model)) => model.clone().into(),
                        _ => self
                            .settings
                            .read(cx)
                            .state()
                            .selected_model
                            .clone()
                            .unwrap_or_else(|| "model".into()),
                    };
                    self.set_model_status(ModelStatus::Loading { model }, cx);
                }
                _ => {}
            },
            RuntimeConfigurationEvent::OperationFinished(operation) => match operation {
                RuntimeConfigurationOperation::DownloadModel => {
                    self.model_operation_in_progress = None;
                    if let Some(PendingModelSetup::Downloading(model)) =
                        self.pending_model_setup.clone()
                    {
                        self.pending_model_setup =
                            Some(PendingModelSetup::Selecting(model.clone()));
                        if let Err(error) = self.runtime.select_model(model) {
                            self.pending_model_setup = None;
                            self.ui_notice = Some(error);
                        }
                    }
                }
                RuntimeConfigurationOperation::SelectModel => {
                    self.model_operation_in_progress = None;
                    if matches!(
                        self.pending_model_setup,
                        Some(PendingModelSetup::Selecting(_))
                    ) {
                        self.pending_model_setup = None;
                        if self.onboarding.read(cx).state().step == OnboardingStep::Model {
                            self.advance_onboarding_to(OnboardingStep::Microphone, cx);
                        }
                    }
                }
                RuntimeConfigurationOperation::InstallCommandTemplate
                    if self.pending_template_installs > 0 =>
                {
                    self.pending_template_installs -= 1;
                    self.finish_template_install_batch_if_ready();
                }
                _ => {}
            },
            RuntimeConfigurationEvent::OperationFailed { operation, message } => {
                self.ui_notice = Some(message.clone());
                match operation {
                    RuntimeConfigurationOperation::DownloadModel
                    | RuntimeConfigurationOperation::SelectModel => {
                        self.model_operation_in_progress = None;
                        self.pending_model_setup = None;
                        let model = self.settings.read(cx).state().selected_model.clone();
                        self.set_model_status(
                            ModelStatus::Error {
                                model,
                                message: message.into(),
                            },
                            cx,
                        );
                    }
                    RuntimeConfigurationOperation::InstallCommandTemplate => {
                        self.template_install_failed = true;
                        if self.pending_template_installs > 0 {
                            self.pending_template_installs -= 1;
                        }
                        self.finish_template_install_batch_if_ready();
                    }
                    RuntimeConfigurationOperation::SelectMicrophone => {
                        let mut state = self.onboarding.read(cx).state().clone();
                        state.microphone_status = MicrophoneStatus::Error {
                            name: state.selected_microphone.clone(),
                            message: message.into(),
                        };
                        self.onboarding
                            .update(cx, |view, cx| view.set_state(state, cx));
                    }
                    _ => {}
                }
            }
        }
    }

    fn finish_template_install_batch_if_ready(&mut self) {
        if self.pending_template_installs != 0 || !self.complete_after_templates {
            return;
        }
        self.complete_after_templates = false;
        if self.template_install_failed {
            self.template_install_failed = false;
            self.ui_notice = Some(
                "One or more command templates could not be installed. Review the error and retry setup."
                    .to_string(),
            );
            return;
        }
        if let Err(error) = self.runtime.mark_onboarding_complete() {
            self.ui_notice = Some(error);
        }
    }

    fn apply_microphone_test_event(
        &mut self,
        event: RuntimeMicrophoneTestEvent,
        cx: &mut Context<Self>,
    ) {
        let mut state = self.onboarding.read(cx).state().clone();
        let name = state
            .selected_microphone
            .clone()
            .unwrap_or_else(|| "Selected microphone".into());
        match event {
            RuntimeMicrophoneTestEvent::Started => {
                state.microphone_status = MicrophoneStatus::Testing { name, level: 0.0 };
            }
            RuntimeMicrophoneTestEvent::Level(level) => {
                state.microphone_status = MicrophoneStatus::Testing { name, level };
            }
            RuntimeMicrophoneTestEvent::Finished { peak_level } => {
                state.microphone_status = if peak_level >= MICROPHONE_SIGNAL_THRESHOLD {
                    MicrophoneStatus::Ready { name }
                } else {
                    MicrophoneStatus::Error {
                        name: Some(name),
                        message: "No microphone signal was detected. Speak during the test or choose another input."
                            .into(),
                    }
                };
            }
            RuntimeMicrophoneTestEvent::Error(message) => {
                state.microphone_status = MicrophoneStatus::Error {
                    name: Some(name),
                    message: message.into(),
                };
            }
        }
        self.onboarding
            .update(cx, |view, cx| view.set_state(state, cx));
    }

    fn apply_ui_action(&mut self, action: NativeUiAction, cx: &mut Context<Self>) {
        match action {
            NativeUiAction::Onboarding(action) => self.apply_onboarding_action(action, cx),
            NativeUiAction::Settings(action) => self.apply_settings_action(action, cx),
            NativeUiAction::Dashboard(action) => self.apply_dashboard_action(action),
            NativeUiAction::Conversation(action) => self.apply_conversation_action(action),
            NativeUiAction::Commands(action) => self.apply_commands_action(action, cx),
            NativeUiAction::Clipboard(action) => self.apply_clipboard_action(action, cx),
            NativeUiAction::Integrations(action) => self.apply_integrations_action(action),
            NativeUiAction::Dictionary(action) => self.apply_dictionary_action(action, cx),
            NativeUiAction::Snippets(action) => self.apply_snippets_action(action, cx),
            NativeUiAction::Style(action) => self.apply_style_action(action),
        }
    }

    fn apply_onboarding_action(&mut self, action: OnboardingAction, cx: &mut Context<Self>) {
        match action {
            OnboardingAction::StepChanged(_) | OnboardingAction::ModelSelected(_) => {}
            OnboardingAction::ModelContinueRequested(model) => {
                let model = model.to_string();
                self.pending_model_setup = Some(PendingModelSetup::Selecting(model.clone()));
                if let Err(error) = self.runtime.select_model(model) {
                    self.pending_model_setup = None;
                    self.ui_notice = Some(error);
                }
            }
            OnboardingAction::DownloadModelRequested(model) => {
                let model = model.to_string();
                self.pending_model_setup = Some(PendingModelSetup::Downloading(model.clone()));
                if let Err(error) = self.runtime.download_model(model) {
                    self.pending_model_setup = None;
                    self.ui_notice = Some(error);
                }
            }
            OnboardingAction::MicrophoneSelected(name) => {
                if let Err(error) = self.runtime.select_microphone(name.to_string()) {
                    self.ui_notice = Some(error);
                }
            }
            OnboardingAction::TestMicrophoneRequested => {
                if let Err(error) = self.runtime.start_microphone_test() {
                    self.ui_notice = Some(error);
                }
            }
            OnboardingAction::CommandTemplateToggled { .. } => {}
            OnboardingAction::CompleteRequested => {
                let template_ids = self
                    .onboarding
                    .read(cx)
                    .state()
                    .selected_templates
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                self.template_install_failed = false;
                self.pending_template_installs = template_ids.len();
                self.complete_after_templates = true;
                if template_ids.is_empty() {
                    self.finish_template_install_batch_if_ready();
                    return;
                }
                for id in template_ids {
                    if let Err(error) = self.runtime.install_command_template(id) {
                        self.template_install_failed = true;
                        self.pending_template_installs =
                            self.pending_template_installs.saturating_sub(1);
                        self.ui_notice = Some(error);
                    }
                }
                self.finish_template_install_batch_if_ready();
            }
        }
    }

    fn apply_settings_action(&mut self, action: SettingsAction, cx: &mut Context<Self>) {
        match action {
            SettingsAction::CloseRequested => self.settings_open = false,
            SettingsAction::ChangeShortcut { kind, shortcut } => {
                let result = match kind {
                    ShortcutKind::HoldToTalk => {
                        self.runtime.set_trigger_hotkey(shortcut.to_string())
                    }
                    ShortcutKind::Assistant => {
                        self.runtime.set_assistant_hotkey(shortcut.to_string())
                    }
                };
                if let Err(error) = result {
                    self.ui_notice = Some(error);
                }
            }
            SettingsAction::ChangeLanguage { kind, language } => {
                let Some(snapshot) = self.configuration_snapshot.as_ref() else {
                    self.ui_notice = Some("Native settings are still loading.".to_string());
                    return;
                };
                let mut source = snapshot.config.language_preferences.source_language.clone();
                let mut target = snapshot.config.language_preferences.target_language.clone();
                match kind {
                    SettingsLanguageKind::Source => source = language.to_string(),
                    SettingsLanguageKind::Target => target = language.to_string(),
                }
                if let Err(error) = self.runtime.set_language_preferences(source, target) {
                    self.ui_notice = Some(error);
                }
            }
            SettingsAction::SelectMicrophone(name) => {
                if let Err(error) = self.runtime.select_microphone(name.to_string()) {
                    self.ui_notice = Some(error);
                }
            }
            SettingsAction::ToggleStartOnLogin(enabled) => {
                let previous_shell_state = self.shell.auto_start_status().ok();
                match self.shell.set_auto_start_enabled(enabled) {
                    Ok(shell_state) => {
                        if let Err(error) = self.runtime.set_auto_start(shell_state.requested()) {
                            if let Some(previous) = previous_shell_state {
                                let _ = self.shell.set_auto_start_enabled(previous.requested());
                            }
                            self.ui_notice = Some(error);
                        } else if shell_state == AutoStartState::RequiresApproval {
                            self.ui_notice = Some(
                                "Start on login is requested, but macOS still requires approval in System Settings."
                                    .to_string(),
                            );
                        } else {
                            self.ui_notice = Some(if shell_state.launches_automatically() {
                                "ListenOS will start automatically when you log in.".to_string()
                            } else {
                                "ListenOS will no longer start automatically when you log in."
                                    .to_string()
                            });
                        }
                    }
                    Err(error) => self.ui_notice = Some(error.to_string()),
                }
            }
            SettingsAction::ToggleShowInTray(enabled) => {
                match self.shell.set_tray_enabled(enabled) {
                    Ok(()) => {
                        if let Some(snapshot) = self.configuration_snapshot.clone() {
                            self.apply_configuration_snapshot(snapshot, cx);
                        }
                    }
                    Err(error) => self.ui_notice = Some(error.to_string()),
                }
            }
            SettingsAction::ToggleGpuAcceleration(enabled) => {
                if let Err(error) = self.runtime.set_use_gpu(enabled) {
                    self.ui_notice = Some(error);
                }
            }
            SettingsAction::CheckForUpdates => {
                if let Err(error) = self.runtime.check_for_updates() {
                    self.update_checking = false;
                    self.update_status = error;
                    if let Some(snapshot) = self.configuration_snapshot.clone() {
                        self.apply_configuration_snapshot(snapshot, cx);
                    }
                }
            }
            SettingsAction::SelectModel(model) => {
                if let Err(error) = self.runtime.select_model(model.to_string()) {
                    self.ui_notice = Some(error);
                }
            }
            SettingsAction::DownloadModel(model) => {
                if let Err(error) = self.runtime.download_model(model.to_string()) {
                    self.ui_notice = Some(error);
                }
            }
            SettingsAction::SetVibeCodingConfig(config) => {
                if let Err(error) = self.runtime.set_vibe_coding_config(config) {
                    self.ui_notice = Some(error);
                }
            }
        }
    }

    fn activity_content(&self, group_index: usize, row_index: usize) -> Option<String> {
        self.current_activity()
            .get(group_index)
            .and_then(|group| group.rows.get(row_index))
            .map(|row| row.content.to_string())
    }

    fn ensure_dictionary_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dictionary_search_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(self.dictionary_search.clone())
                    .placeholder("Search...")
            });
            let subscription =
                cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    this.dictionary_search = input.read(cx).value().to_string();
                    cx.notify();
                });
            self.dictionary_search_input = Some(input);
            self.dictionary_search_subscription = Some(subscription);
        }
        if self.dictionary_word_input.is_none() {
            self.dictionary_word_input =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder("e.g., Kubernetes")));
        }
        if self.dictionary_phonetic_input.is_none() {
            self.dictionary_phonetic_input = Some(
                cx.new(|cx| InputState::new(window, cx).placeholder("e.g., koo-ber-nee-tees")),
            );
        }

        if let Some(value) = self.dictionary_search_pending_value.take()
            && let Some(input) = &self.dictionary_search_input
        {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        if let Some((word, phonetic)) = self.dictionary_editor_pending_values.take() {
            if let Some(input) = &self.dictionary_word_input {
                input.update(cx, |state, cx| state.set_value(word, window, cx));
            }
            if let Some(input) = &self.dictionary_phonetic_input {
                input.update(cx, |state, cx| state.set_value(phonetic, window, cx));
            }
        }
    }

    fn ensure_clipboard_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot_value = self
            .dashboard_snapshot
            .as_ref()
            .map(|snapshot| snapshot.clipboard.clone())
            .unwrap_or_default();

        if self.clipboard_input.is_none() {
            self.clipboard_editor_source = snapshot_value.clone();
            self.clipboard_editor_value = snapshot_value.clone();
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(8)
                    .default_value(snapshot_value)
                    .placeholder("Clipboard is empty")
            });
            let subscription =
                cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    this.clipboard_editor_value = input.read(cx).value().to_string();
                    cx.notify();
                });
            self.clipboard_input = Some(input);
            self.clipboard_input_subscription = Some(subscription);
            return;
        }

        let Some(input) = &self.clipboard_input else {
            return;
        };
        let current_value = input.read(cx).value().to_string();
        if current_value == snapshot_value {
            self.clipboard_editor_source = snapshot_value;
            self.clipboard_editor_value = current_value;
        } else if current_value == self.clipboard_editor_source
            && snapshot_value != self.clipboard_editor_source
        {
            input.update(cx, |state, cx| {
                state.set_value(snapshot_value.clone(), window, cx)
            });
            self.clipboard_editor_source = snapshot_value.clone();
            self.clipboard_editor_value = snapshot_value;
        }
    }

    fn ensure_snippets_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.snippets_search_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(self.snippets_search.clone())
                    .placeholder("Search...")
            });
            let subscription =
                cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    this.snippets_search = input.read(cx).value().to_string();
                    cx.notify();
                });
            self.snippets_search_input = Some(input);
            self.snippets_search_subscription = Some(subscription);
        }
        if self.snippets_trigger_input.is_none() {
            self.snippets_trigger_input =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder("e.g., my email")));
        }
        if self.snippets_expansion_input.is_none() {
            self.snippets_expansion_input = Some(cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(3)
                    .placeholder("e.g., example@email.com")
            }));
        }

        if let Some(value) = self.snippets_search_pending_value.take()
            && let Some(input) = &self.snippets_search_input
        {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        if let Some((trigger, expansion)) = self.snippets_editor_pending_values.take() {
            if let Some(input) = &self.snippets_trigger_input {
                input.update(cx, |state, cx| state.set_value(trigger, window, cx));
            }
            if let Some(input) = &self.snippets_expansion_input {
                input.update(cx, |state, cx| state.set_value(expansion, window, cx));
            }
        }
    }

    fn ensure_command_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_name_input.is_none() {
            self.command_name_input =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder("Morning Routine")));
        }
        if self.command_trigger_input.is_none() {
            self.command_trigger_input =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder("morning routine")));
        }
        if self.command_description_input.is_none() {
            self.command_description_input = Some(cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(2)
                    .placeholder("What this command does")
            }));
        }
        if let Some((name, trigger, description, action_steps)) =
            self.command_editor_pending_values.take()
        {
            if let Some(input) = &self.command_name_input {
                input.update(cx, |state, cx| state.set_value(name, window, cx));
            }
            if let Some(input) = &self.command_trigger_input {
                input.update(cx, |state, cx| state.set_value(trigger, window, cx));
            }
            if let Some(input) = &self.command_description_input {
                input.update(cx, |state, cx| state.set_value(description, window, cx));
            }
            self.command_action_inputs.clear();
            self.command_action_inputs.extend(
                action_steps
                    .into_iter()
                    .map(|step| Self::command_action_input_state(step, window, cx)),
            );
        }
        if let Some(action_type) = self.command_pending_action_type.take() {
            let delay_ms = if self.command_action_inputs.is_empty() {
                0
            } else {
                500
            };
            self.command_action_inputs
                .push(Self::command_action_input_state(
                    ActionStep::new(&action_type, serde_json::json!({})).with_delay(delay_ms),
                    window,
                    cx,
                ));
        }
    }

    fn command_action_input_state(
        step: ActionStep,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> CommandActionInputState {
        let action_type = step.action_type.clone();
        let payload = serde_json::to_string(&step.payload).unwrap_or_else(|_| "{}".to_string());
        let delay_ms = step.delay_ms.to_string();
        CommandActionInputState {
            id: step.id,
            description: step.description,
            action_type: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(action_type)
                    .placeholder("action type")
            }),
            payload: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(payload)
                    .placeholder("{}")
            }),
            delay_ms: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(delay_ms)
                    .placeholder("0")
            }),
        }
    }

    fn open_command_editor(&mut self, command: CustomCommand) {
        self.command_editor_pending_values = Some((
            command.name.clone(),
            command.trigger_phrase.clone(),
            command.description.clone(),
            command.actions.clone(),
        ));
        self.command_pending_action_type = None;
        self.command_editor = Some(command);
        self.ui_notice = None;
    }

    fn apply_dashboard_action(&mut self, action: DashboardAction) {
        match action {
            DashboardAction::FeatureTip => {
                self.ui_notice = Some(format!(
                    "Hold {} to dictate locally; release to process without a web IPC hop.",
                    self.hold_to_talk_shortcut_label()
                ));
            }
            DashboardAction::CopyActivity {
                group_index,
                row_index,
            } => {
                if let Some(content) = self.activity_content(group_index, row_index)
                    && let Err(error) = self.runtime.set_clipboard(content)
                {
                    self.ui_notice = Some(error);
                }
            }
        }
    }

    fn apply_conversation_action(&mut self, action: ConversationAction) {
        let result = match action {
            ConversationAction::NewSession => self.runtime.new_conversation_session(),
            ConversationAction::Clear => self.runtime.clear_conversation(),
            ConversationAction::CopyActivity {
                group_index,
                row_index,
            } => self
                .activity_content(group_index, row_index)
                .map(|content| self.runtime.set_clipboard(content))
                .unwrap_or(Ok(())),
        };
        if let Err(error) = result {
            self.ui_notice = Some(error);
        }
    }

    fn apply_commands_action(&mut self, action: CommandsAction, cx: &mut Context<Self>) {
        match action {
            CommandsAction::SelectTab(tab) => self.commands_tab = tab,
            CommandsAction::ToggleEnabled { index, enabled } => {
                let id = self
                    .dashboard_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.custom_commands.get(index))
                    .map(|command| command.id.clone());
                if let Some(id) = id
                    && let Err(error) = self.runtime.set_custom_command_enabled(id, enabled)
                {
                    self.ui_notice = Some(error);
                }
            }
            CommandsAction::Delete { index } => {
                let id = self
                    .dashboard_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.custom_commands.get(index))
                    .map(|command| command.id.clone());
                if let Some(id) = id {
                    self.pending_destructive_delete = Some(PendingDestructiveDelete::Command(id));
                }
            }
            CommandsAction::UseTemplate { index } => {
                let template = self
                    .dashboard_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.command_templates.get(index))
                    .cloned();
                if let Some(mut command) = template {
                    command.id = uuid::Uuid::new_v4().to_string();
                    command.enabled = true;
                    command.created_at = chrono::Utc::now();
                    command.last_used = None;
                    command.use_count = 0;
                    self.open_command_editor(command);
                }
            }
            CommandsAction::Import => {
                let picker = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("Import ListenOS commands".into()),
                });
                cx.spawn(async move |this, cx| {
                    let paths = match picker.await {
                        Ok(Ok(paths)) => paths,
                        Ok(Err(error)) => {
                            let _ = this.update(cx, |this, cx| {
                                this.ui_notice =
                                    Some(format!("Could not open file picker: {error}"));
                                cx.notify();
                            });
                            return;
                        }
                        Err(error) => {
                            let _ = this.update(cx, |this, cx| {
                                this.ui_notice =
                                    Some(format!("Command import was interrupted: {error}"));
                                cx.notify();
                            });
                            return;
                        }
                    };
                    let Some(path) = paths.and_then(|mut paths| paths.drain(..).next()) else {
                        return;
                    };
                    let imported = cx
                        .background_spawn(async move {
                            let json = std::fs::read_to_string(&path).map_err(|error| {
                                format!("Could not read {}: {error}", path.display())
                            })?;
                            serde_json::from_str::<Vec<CustomCommand>>(&json)
                                .map_err(|error| format!("Invalid ListenOS command file: {error}"))
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        match imported {
                            Ok(commands) => {
                                if let Err(error) = this.runtime.import_custom_commands(commands) {
                                    this.ui_notice = Some(error);
                                }
                            }
                            Err(error) => this.ui_notice = Some(error),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            CommandsAction::Export => {
                let commands = self
                    .dashboard_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.custom_commands.clone())
                    .unwrap_or_default();
                let directory = std::env::current_dir().unwrap_or_default();
                let picker = cx.prompt_for_new_path(&directory, Some("listenos-commands.json"));
                cx.spawn(async move |this, cx| {
                    let path = match picker.await {
                        Ok(Ok(path)) => path,
                        Ok(Err(error)) => {
                            let _ = this.update(cx, |this, cx| {
                                this.ui_notice =
                                    Some(format!("Could not open save dialog: {error}"));
                                cx.notify();
                            });
                            return;
                        }
                        Err(error) => {
                            let _ = this.update(cx, |this, cx| {
                                this.ui_notice =
                                    Some(format!("Command export was interrupted: {error}"));
                                cx.notify();
                            });
                            return;
                        }
                    };
                    let Some(path) = path else {
                        return;
                    };
                    let result = cx
                        .background_spawn(async move {
                            let json =
                                serde_json::to_string_pretty(&commands).map_err(|error| {
                                    format!("Could not serialize commands: {error}")
                                })?;
                            std::fs::write(&path, json).map_err(|error| {
                                format!("Could not write {}: {error}", path.display())
                            })
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.ui_notice = Some(match result {
                            Ok(()) => "Custom commands exported".to_string(),
                            Err(error) => error,
                        });
                        cx.notify();
                    });
                })
                .detach();
            }
            CommandsAction::NewCommand => {
                self.open_command_editor(CustomCommand {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: String::new(),
                    trigger_phrase: String::new(),
                    description: String::new(),
                    actions: Vec::new(),
                    enabled: true,
                    created_at: chrono::Utc::now(),
                    last_used: None,
                    use_count: 0,
                });
            }
            CommandsAction::Edit { index } => {
                let command = self
                    .dashboard_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.custom_commands.get(index))
                    .cloned();
                if let Some(command) = command {
                    self.open_command_editor(command);
                }
            }
            CommandsAction::CancelEditor => {
                self.command_editor = None;
                self.command_action_inputs.clear();
                self.command_pending_action_type = None;
                self.ui_notice = None;
            }
            CommandsAction::AddEditorAction { action_type } => {
                self.command_pending_action_type = Some(action_type.to_string());
            }
            CommandsAction::RemoveEditorAction { index } => {
                if index < self.command_action_inputs.len() {
                    self.command_action_inputs.remove(index);
                }
            }
            CommandsAction::SaveEditor => {
                let Some(mut command) = self.command_editor.clone() else {
                    return;
                };
                let Some(name_input) = &self.command_name_input else {
                    return;
                };
                let Some(trigger_input) = &self.command_trigger_input else {
                    return;
                };
                let Some(description_input) = &self.command_description_input else {
                    return;
                };
                let name = name_input.read(cx).value().trim().to_string();
                let trigger = trigger_input.read(cx).value().trim().to_lowercase();
                if name.is_empty() || trigger.is_empty() {
                    self.ui_notice =
                        Some("Command name and trigger phrase are required.".to_string());
                    return;
                }
                let description = description_input.read(cx).value().trim().to_string();
                let mut actions = Vec::with_capacity(self.command_action_inputs.len());
                for (index, row) in self.command_action_inputs.iter().enumerate() {
                    let action_type = row.action_type.read(cx).value().trim().to_string();
                    if action_type.is_empty() {
                        self.ui_notice = Some(format!("Action {} needs a type.", index + 1));
                        return;
                    }
                    let payload_text = row.payload.read(cx).value().trim().to_string();
                    let payload = match serde_json::from_str::<serde_json::Value>(&payload_text) {
                        Ok(payload) => payload,
                        Err(error) => {
                            self.ui_notice = Some(format!(
                                "Action {} has invalid payload JSON: {error}",
                                index + 1
                            ));
                            return;
                        }
                    };
                    let delay_text = row.delay_ms.read(cx).value().trim().to_string();
                    let delay_ms = match delay_text.parse::<u32>() {
                        Ok(delay_ms) => delay_ms,
                        Err(_) => {
                            self.ui_notice = Some(format!(
                                "Action {} delay must be a non-negative whole number.",
                                index + 1
                            ));
                            return;
                        }
                    };
                    actions.push(ActionStep {
                        id: row.id.clone(),
                        action_type,
                        payload,
                        delay_ms,
                        description: row.description.clone(),
                    });
                }
                command.name = name;
                command.trigger_phrase = trigger;
                command.description = description;
                command.actions = actions;
                match self.runtime.save_custom_command(command) {
                    Ok(()) => {
                        self.command_editor = None;
                        self.command_action_inputs.clear();
                        self.command_pending_action_type = None;
                        self.ui_notice = None;
                    }
                    Err(error) => self.ui_notice = Some(error),
                }
            }
        }
    }

    fn transform_clipboard(action: ClipboardQuickAction, content: &str) -> String {
        match action {
            ClipboardQuickAction::BulletList => content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| format!("• {}", line.trim()))
                .collect::<Vec<_>>()
                .join("\n"),
            ClipboardQuickAction::NumberedList => content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .enumerate()
                .map(|(index, line)| format!("{}. {}", index + 1, line.trim()))
                .collect::<Vec<_>>()
                .join("\n"),
            ClipboardQuickAction::CleanUpText => {
                content.split_whitespace().collect::<Vec<_>>().join(" ")
            }
            ClipboardQuickAction::Uppercase => content.to_uppercase(),
            ClipboardQuickAction::Lowercase => content.to_lowercase(),
            ClipboardQuickAction::TitleCase => content
                .split_whitespace()
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        Some(first) => {
                            format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase())
                        }
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    fn apply_clipboard_action(&mut self, action: ClipboardAction, cx: &mut Context<Self>) {
        let result = match action {
            ClipboardAction::SelectTab(tab) => {
                self.clipboard_tab = tab;
                Ok(())
            }
            ClipboardAction::Refresh => self.runtime.refresh_dashboard_data(),
            ClipboardAction::UpdateCurrent => self
                .clipboard_input
                .as_ref()
                .map(|input| {
                    self.runtime
                        .set_clipboard(input.read(cx).value().to_string())
                })
                .unwrap_or_else(|| Err("Clipboard editor is unavailable.".to_string())),
            ClipboardAction::QuickAction(action) => {
                let current = self
                    .clipboard_input
                    .as_ref()
                    .map(|input| input.read(cx).value().to_string())
                    .unwrap_or_else(|| {
                        self.dashboard_snapshot
                            .as_ref()
                            .map(|snapshot| snapshot.clipboard.clone())
                            .unwrap_or_default()
                    });
                if current.trim().is_empty() {
                    Ok(())
                } else {
                    self.runtime
                        .set_clipboard(Self::transform_clipboard(action, &current))
                }
            }
            ClipboardAction::CopyHistory { index } => self
                .dashboard_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.clipboard_history.get(index))
                .map(|entry| self.runtime.set_clipboard(entry.content.clone()))
                .unwrap_or(Ok(())),
        };
        if let Err(error) = result {
            self.ui_notice = Some(error);
        }
    }

    fn apply_integrations_action(&mut self, action: IntegrationsAction) {
        match action {
            IntegrationsAction::ToggleEnabled { index, enabled } => {
                let name = self
                    .dashboard_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.integrations.get(index))
                    .map(|integration| integration.name.clone());
                if let Some(name) = name
                    && let Err(error) = self.runtime.set_integration_enabled(name, enabled)
                {
                    self.ui_notice = Some(error);
                }
            }
            IntegrationsAction::ToggleExpanded { index, expanded } => {
                let name = self
                    .dashboard_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.integrations.get(index))
                    .map(|integration| integration.name.clone());
                if let Some(name) = name {
                    if expanded {
                        self.expanded_integrations.insert(name);
                    } else {
                        self.expanded_integrations.remove(&name);
                    }
                }
            }
        }
    }

    fn filtered_dictionary_entries(&self) -> Vec<&voice_os_lib::DictionaryWord> {
        let Some(snapshot) = &self.dashboard_snapshot else {
            return Vec::new();
        };
        let query = self.dictionary_search.to_lowercase();
        snapshot
            .dictionary_words
            .iter()
            .filter(|entry| match self.dictionary_tab {
                CollectionTab::All => true,
                CollectionTab::Personal => entry.category.eq_ignore_ascii_case("personal"),
                CollectionTab::Shared => entry.category.eq_ignore_ascii_case("shared"),
            })
            .filter(|entry| query.is_empty() || entry.word.to_lowercase().contains(&query))
            .collect()
    }

    fn apply_dictionary_action(&mut self, action: DictionaryAction, cx: &mut Context<Self>) {
        match action {
            DictionaryAction::SelectTab(tab) => self.dictionary_tab = tab,
            DictionaryAction::SearchChanged(query) => {
                self.dictionary_search = query.to_string();
                self.dictionary_search_pending_value = Some(self.dictionary_search.clone());
            }
            DictionaryAction::Delete { index } => {
                let id = self
                    .filtered_dictionary_entries()
                    .get(index)
                    .map(|entry| entry.id.clone());
                if let Some(id) = id {
                    self.pending_destructive_delete =
                        Some(PendingDestructiveDelete::DictionaryWord(id));
                }
            }
            DictionaryAction::AddNew => {
                self.dictionary_editor = Some(DictionaryEditorTarget::New);
                self.dictionary_editor_pending_values = Some((String::new(), String::new()));
                self.ui_notice = None;
            }
            DictionaryAction::Edit { index } => {
                let entry = self
                    .filtered_dictionary_entries()
                    .get(index)
                    .map(|entry| (*entry).clone());
                if let Some(entry) = entry {
                    self.dictionary_editor =
                        Some(DictionaryEditorTarget::Existing(entry.id.clone()));
                    self.dictionary_editor_pending_values = Some((
                        entry.word.clone(),
                        entry.phonetic.clone().unwrap_or_default(),
                    ));
                    self.ui_notice = None;
                }
            }
            DictionaryAction::CancelEditor => {
                self.dictionary_editor = None;
                self.ui_notice = None;
            }
            DictionaryAction::SaveEditor => {
                let Some(target) = self.dictionary_editor.clone() else {
                    return;
                };
                let Some(word_input) = &self.dictionary_word_input else {
                    return;
                };
                let word = word_input.read(cx).value().trim().to_string();
                if word.is_empty() {
                    self.ui_notice = Some("Dictionary word or phrase is required.".to_string());
                    return;
                }
                let phonetic = self.dictionary_phonetic_input.as_ref().and_then(|input| {
                    let value = input.read(cx).value().trim().to_string();
                    (!value.is_empty()).then_some(value)
                });
                let result = match target {
                    DictionaryEditorTarget::New => self.runtime.add_dictionary_word(word, false),
                    DictionaryEditorTarget::Existing(id) => {
                        self.runtime.update_dictionary_word(id, word, phonetic)
                    }
                };
                match result {
                    Ok(()) => {
                        self.dictionary_editor = None;
                        self.ui_notice = None;
                    }
                    Err(error) => self.ui_notice = Some(error),
                }
            }
        }
    }

    fn filtered_snippet_entries(&self) -> Vec<&voice_os_lib::Snippet> {
        let Some(snapshot) = &self.dashboard_snapshot else {
            return Vec::new();
        };
        let query = self.snippets_search.to_lowercase();
        snapshot
            .snippets
            .iter()
            .filter(|entry| match self.snippets_tab {
                CollectionTab::All => true,
                CollectionTab::Personal => entry.category.eq_ignore_ascii_case("personal"),
                CollectionTab::Shared => entry.category.eq_ignore_ascii_case("shared"),
            })
            .filter(|entry| {
                query.is_empty()
                    || entry.trigger.to_lowercase().contains(&query)
                    || entry.expansion.to_lowercase().contains(&query)
            })
            .collect()
    }

    fn apply_snippets_action(&mut self, action: SnippetsAction, cx: &mut Context<Self>) {
        match action {
            SnippetsAction::SelectTab(tab) => self.snippets_tab = tab,
            SnippetsAction::SearchChanged(query) => {
                self.snippets_search = query.to_string();
                self.snippets_search_pending_value = Some(self.snippets_search.clone());
            }
            SnippetsAction::Delete { index } => {
                let id = self
                    .filtered_snippet_entries()
                    .get(index)
                    .map(|entry| entry.id.clone());
                if let Some(id) = id {
                    self.pending_destructive_delete = Some(PendingDestructiveDelete::Snippet(id));
                }
            }
            SnippetsAction::AddNew => {
                self.snippets_editor = Some(SnippetEditorTarget::New);
                self.snippets_editor_pending_values = Some((String::new(), String::new()));
                self.ui_notice = None;
            }
            SnippetsAction::Edit { index } => {
                let entry = self
                    .filtered_snippet_entries()
                    .get(index)
                    .map(|entry| (*entry).clone());
                if let Some(entry) = entry {
                    self.snippets_editor = Some(SnippetEditorTarget::Existing(entry.id.clone()));
                    self.snippets_editor_pending_values =
                        Some((entry.trigger.clone(), entry.expansion.clone()));
                    self.ui_notice = None;
                }
            }
            SnippetsAction::CancelEditor => {
                self.snippets_editor = None;
                self.ui_notice = None;
            }
            SnippetsAction::SaveEditor => {
                let Some(target) = self.snippets_editor.clone() else {
                    return;
                };
                let Some(trigger_input) = &self.snippets_trigger_input else {
                    return;
                };
                let Some(expansion_input) = &self.snippets_expansion_input else {
                    return;
                };
                let trigger = trigger_input.read(cx).value().trim().to_string();
                let expansion = expansion_input.read(cx).value().trim().to_string();
                if trigger.is_empty() || expansion.is_empty() {
                    self.ui_notice =
                        Some("Snippet trigger and expansion are required.".to_string());
                    return;
                }
                let result = match target {
                    SnippetEditorTarget::New => self.runtime.create_snippet(trigger, expansion),
                    SnippetEditorTarget::Existing(id) => {
                        self.runtime.update_snippet(id, trigger, expansion)
                    }
                };
                match result {
                    Ok(()) => {
                        self.snippets_editor = None;
                        self.ui_notice = None;
                    }
                    Err(error) => self.ui_notice = Some(error),
                }
            }
        }
    }

    fn runtime_style_context(context: StyleContext) -> RuntimeDictationStyleContext {
        match context {
            StyleContext::Personal => RuntimeDictationStyleContext::Personal,
            StyleContext::Work => RuntimeDictationStyleContext::Work,
            StyleContext::Email => RuntimeDictationStyleContext::Email,
            StyleContext::Other => RuntimeDictationStyleContext::Other,
        }
    }

    fn dictation_style(tone: StyleTone) -> DictationStyle {
        match tone {
            StyleTone::Formal => DictationStyle::Formal,
            StyleTone::Casual => DictationStyle::Casual,
            StyleTone::VeryCasual => DictationStyle::VeryCasual,
        }
    }

    fn apply_style_action(&mut self, action: StyleAction) {
        match action {
            StyleAction::SelectContext(context) => self.style_context = context,
            StyleAction::SelectTone(tone) => {
                if let Err(error) = self.runtime.set_dictation_style(
                    Self::runtime_style_context(self.style_context),
                    Self::dictation_style(tone),
                ) {
                    self.ui_notice = Some(error);
                }
            }
        }
    }

    fn nav_item(
        &self,
        section: DashboardSection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let selected = self.section == section;
        let tokens = self.tokens;

        div()
            .id(("nav-item", section.id()))
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .text_color(if selected {
                tokens.text
            } else {
                tokens.text_muted
            })
            .bg(if selected {
                tokens.accent
            } else {
                transparent()
            })
            .hover(|style| style.bg(tokens.accent).text_color(tokens.text))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.section = section;
                cx.notify();
            }))
            .child(section.label())
    }

    fn content_card(&self, title: &'static str, value: String) -> impl IntoElement {
        let tokens = self.tokens;
        div()
            .flex_1()
            .min_w(px(180.0))
            .rounded_lg()
            .border_1()
            .border_color(tokens.border)
            .bg(tokens.muted)
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_xs().text_color(tokens.text_muted).child(title))
            .child(div().text_xl().text_color(tokens.text).child(value))
    }

    fn confirmation_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        (self.overlay == OverlayState::ConfirmationRequired).then(|| {
            let tokens = self.tokens;
            div()
                .w_full()
                .rounded_lg()
                .border_1()
                .border_color(tokens.warning)
                .bg(tokens.muted)
                .p_4()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .min_w(px(0.0))
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .text_color(tokens.text)
                                .child("Confirm action"),
                        )
                        .child(
                            div()
                                .line_clamp(3)
                                .text_xs()
                                .text_color(tokens.text_muted)
                                .child(self.runtime_ui.overlay_detail.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            ListenOsButton::new("confirm-pending-action", "Confirm", tokens)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Err(error) = this.runtime.confirm_pending() {
                                        this.apply_runtime_event(RuntimeEvent::Error(error), cx);
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(
                            ListenOsButton::new("cancel-pending-action", "Cancel", tokens)
                                .variant(ListenOsButtonVariant::Secondary)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Err(error) = this.runtime.cancel_pending() {
                                        this.apply_runtime_event(RuntimeEvent::Error(error), cx);
                                        cx.notify();
                                    }
                                })),
                        ),
                )
                .into_any_element()
        })
    }

    fn destructive_confirmation_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = self.pending_destructive_delete.as_ref()?;
        let prompt = match pending {
            PendingDestructiveDelete::Command(_) => "Are you sure you want to delete this command?",
            PendingDestructiveDelete::DictionaryWord(_) => {
                "Are you sure you want to delete this word?"
            }
            PendingDestructiveDelete::Snippet(_) => "Are you sure you want to delete this snippet?",
        };
        let tokens = self.tokens;
        Some(
            div()
                .w_full()
                .rounded_lg()
                .border_1()
                .border_color(tokens.negative)
                .bg(tokens.muted)
                .p_4()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .min_w(px(0.0))
                        .flex_1()
                        .text_sm()
                        .text_color(tokens.text)
                        .child(prompt),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            ListenOsButton::new("cancel-destructive-delete", "Cancel", tokens)
                                .variant(ListenOsButtonVariant::Secondary)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.pending_destructive_delete = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListenOsButton::new("confirm-destructive-delete", "Delete", tokens)
                                .variant(ListenOsButtonVariant::Destructive)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let Some(pending) = this.pending_destructive_delete.take()
                                    else {
                                        return;
                                    };
                                    let result = match pending {
                                        PendingDestructiveDelete::Command(id) => {
                                            this.runtime.delete_custom_command(id)
                                        }
                                        PendingDestructiveDelete::DictionaryWord(id) => {
                                            this.runtime.delete_dictionary_word(id)
                                        }
                                        PendingDestructiveDelete::Snippet(id) => {
                                            this.runtime.delete_snippet(id)
                                        }
                                    };
                                    if let Err(error) = result {
                                        this.ui_notice = Some(error);
                                    }
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    fn current_activity(&self) -> Vec<ActivityGroup> {
        if let Some(snapshot) = &self.dashboard_snapshot {
            let mut groups: Vec<ActivityGroup> = Vec::new();
            for message in snapshot
                .conversation
                .iter()
                .filter(|message| message.role == Role::User)
            {
                let date_label = message.timestamp.format("%b %d").to_string().to_uppercase();
                let row = ActivityRow {
                    time_label: message.timestamp.format("%H:%M").to_string().into(),
                    content: message.content.clone().into(),
                };
                if let Some(group) = groups
                    .iter_mut()
                    .find(|group| group.date_label.as_ref() == date_label)
                {
                    group.rows.push(row);
                } else {
                    groups.push(ActivityGroup {
                        date_label: date_label.into(),
                        rows: vec![row],
                    });
                }
            }
            if !groups.is_empty() {
                return groups;
            }
        }

        let transcription = self.runtime_ui.last_transcription.trim();
        if transcription.is_empty() || transcription == "No transcription yet" {
            return Vec::new();
        }

        vec![ActivityGroup {
            date_label: "Current session".into(),
            rows: vec![ActivityRow {
                time_label: "Latest".into(),
                content: transcription.to_string().into(),
            }],
        }]
    }

    fn dashboard_stats(&self) -> DashboardStats {
        let Some(snapshot) = &self.dashboard_snapshot else {
            return DashboardStats::default();
        };
        DashboardStats {
            streak_days: 0,
            total_words: snapshot
                .history
                .iter()
                .map(|entry| entry.transcription.text.split_whitespace().count() as u64)
                .sum(),
            today_words: snapshot
                .conversation
                .iter()
                .filter(|message| message.role == Role::User)
                .map(|message| message.content.split_whitespace().count() as u64)
                .sum(),
        }
    }

    fn command_cards(commands: &[voice_os_lib::CustomCommand]) -> Vec<CommandCardProps> {
        commands
            .iter()
            .map(|command| CommandCardProps {
                name: command.name.clone().into(),
                trigger_phrase: command.trigger_phrase.clone().into(),
                description: command.description.clone().into(),
                action_labels: command
                    .actions
                    .iter()
                    .map(|action| {
                        action
                            .description
                            .clone()
                            .unwrap_or_else(|| action.action_type.clone())
                            .into()
                    })
                    .collect(),
                use_count: command.use_count,
                enabled: command.enabled,
            })
            .collect()
    }

    fn clipboard_kind(kind: ClipboardContentType) -> ClipboardContentKind {
        match kind {
            ClipboardContentType::Url => ClipboardContentKind::Url,
            ClipboardContentType::Email => ClipboardContentKind::Email,
            ClipboardContentType::Code => ClipboardContentKind::Code,
            ClipboardContentType::List => ClipboardContentKind::List,
            ClipboardContentType::Text | ClipboardContentType::Unknown => {
                ClipboardContentKind::Text
            }
        }
    }

    fn style_tone(style: DictationStyle) -> StyleTone {
        match style {
            DictationStyle::Formal => StyleTone::Formal,
            DictationStyle::Casual => StyleTone::Casual,
            DictationStyle::VeryCasual => StyleTone::VeryCasual,
        }
    }

    fn route_surface(&self) -> AnyElement {
        let tokens = self.tokens;
        let snapshot = self.dashboard_snapshot.as_ref();
        match self.section {
            DashboardSection::Dashboard => {
                let sender = self.ui_action_tx.clone();
                let actions = DashboardActionProps::new(move |action| {
                    let _ = sender.send(NativeUiAction::Dashboard(action));
                });
                dashboard_view_with_actions(
                    &DashboardProps {
                        greeting: GreetingProps {
                            greeting: "Welcome back".into(),
                            date_label: "Current native session".into(),
                            stats: self.dashboard_stats(),
                        },
                        feature_tip: FeatureTipProps {
                            title: format!("{} is ready", self.runtime_ui.model).into(),
                            description: format!(
                                "Hold {} to dictate locally. Current input: {}.",
                                self.hold_to_talk_shortcut_label(),
                                self.runtime_ui.audio_device
                            )
                            .into(),
                            action_label: None,
                        },
                        recent_activity: self.current_activity(),
                        activity_loading: self.dashboard_loading,
                    },
                    &actions,
                    tokens,
                )
                .into_any_element()
            }
            DashboardSection::Conversation => {
                let sender = self.ui_action_tx.clone();
                let actions = ConversationActionProps::new(move |action| {
                    let _ = sender.send(NativeUiAction::Conversation(action));
                });
                conversation_view_with_actions(
                    &ConversationProps {
                        groups: self.current_activity(),
                        loading: self.dashboard_loading,
                    },
                    &actions,
                    tokens,
                )
                .into_any_element()
            }
            DashboardSection::Commands => {
                let sender = self.ui_action_tx.clone();
                let actions = CommandsActionProps::new(move |action| {
                    let _ = sender.send(NativeUiAction::Commands(action));
                });
                let editor = match (
                    self.command_editor.as_ref(),
                    self.command_name_input.as_ref(),
                    self.command_trigger_input.as_ref(),
                    self.command_description_input.as_ref(),
                ) {
                    (Some(command), Some(name), Some(trigger), Some(description)) => {
                        Some(CommandEditorInputs {
                            editing: snapshot
                                .map(|snapshot| {
                                    snapshot
                                        .custom_commands
                                        .iter()
                                        .any(|existing| existing.id == command.id)
                                })
                                .unwrap_or(false),
                            name: name.clone(),
                            trigger: trigger.clone(),
                            description: description.clone(),
                            action_rows: self
                                .command_action_inputs
                                .iter()
                                .map(|row| CommandActionEditorInputs {
                                    action_type: row.action_type.clone(),
                                    payload: row.payload.clone(),
                                    delay_ms: row.delay_ms.clone(),
                                })
                                .collect(),
                        })
                    }
                    _ => None,
                };
                commands_view_with_actions(
                    &CommandsProps {
                        active_tab: self.commands_tab,
                        commands: snapshot
                            .map(|snapshot| Self::command_cards(&snapshot.custom_commands))
                            .unwrap_or_default(),
                        templates: snapshot
                            .map(|snapshot| Self::command_cards(&snapshot.command_templates))
                            .unwrap_or_default(),
                        loading: self.dashboard_loading,
                    },
                    &actions,
                    editor.as_ref(),
                    tokens,
                )
                .into_any_element()
            }
            DashboardSection::Clipboard => {
                let sender = self.ui_action_tx.clone();
                let actions = ClipboardActionProps::new(move |action| {
                    let _ = sender.send(NativeUiAction::Clipboard(action));
                });
                clipboard_view_with_actions(
                    &ClipboardProps {
                        active_tab: self.clipboard_tab,
                        current_content: self.clipboard_editor_value.clone().into(),
                        history: snapshot
                            .map(|snapshot| {
                                snapshot
                                    .clipboard_history
                                    .iter()
                                    .map(|entry| ClipboardHistoryItem {
                                        content: entry.content.clone().into(),
                                        kind: Self::clipboard_kind(entry.content_type),
                                        word_count: entry.word_count,
                                        char_count: entry.char_count,
                                        timestamp_label: entry
                                            .timestamp
                                            .format("%b %d, %H:%M")
                                            .to_string()
                                            .into(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        loading: self.dashboard_loading,
                    },
                    &actions,
                    self.clipboard_input.as_ref(),
                    tokens,
                )
                .into_any_element()
            }
            DashboardSection::Integrations => {
                let sender = self.ui_action_tx.clone();
                let actions = IntegrationsActionProps::new(move |action| {
                    let _ = sender.send(NativeUiAction::Integrations(action));
                });
                integrations_view_with_actions(
                    &IntegrationsProps {
                        integrations: snapshot
                            .map(|snapshot| {
                                snapshot
                                    .integrations
                                    .iter()
                                    .map(|integration| IntegrationCardProps {
                                        name: integration.name.clone().into(),
                                        description: integration.description.clone().into(),
                                        enabled: integration.enabled,
                                        available: integration.available,
                                        expanded: self
                                            .expanded_integrations
                                            .contains(&integration.name),
                                        actions: integration
                                            .actions
                                            .iter()
                                            .map(|action| IntegrationActionProps {
                                                name: action.name.clone().into(),
                                                description: action.description.clone().into(),
                                                example_phrases: action
                                                    .example_phrases
                                                    .iter()
                                                    .cloned()
                                                    .map(Into::into)
                                                    .collect(),
                                            })
                                            .collect(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        loading: self.dashboard_loading,
                    },
                    &actions,
                    tokens,
                )
                .into_any_element()
            }
            DashboardSection::Dictionary => {
                let sender = self.ui_action_tx.clone();
                let actions = DictionaryActionProps::new(move |action| {
                    let _ = sender.send(NativeUiAction::Dictionary(action));
                });
                let entries = self
                    .filtered_dictionary_entries()
                    .into_iter()
                    .map(|entry| DictionaryEntryProps {
                        word: entry.word.clone().into(),
                        phonetic: entry.phonetic.clone().map(Into::into),
                        auto_learned: entry.is_auto_learned,
                        use_count: entry.use_count,
                    })
                    .collect::<Vec<_>>();
                let editor = match (
                    self.dictionary_editor.as_ref(),
                    self.dictionary_word_input.as_ref(),
                    self.dictionary_phonetic_input.as_ref(),
                ) {
                    (Some(target), Some(word), Some(phonetic)) => Some(DictionaryEditorInputs {
                        editing: matches!(target, DictionaryEditorTarget::Existing(_)),
                        word: word.clone(),
                        phonetic: phonetic.clone(),
                    }),
                    _ => None,
                };
                dictionary_view_with_actions(
                    &DictionaryProps {
                        active_tab: self.dictionary_tab,
                        search_query: self.dictionary_search.clone().into(),
                        show_intro: snapshot
                            .map(|snapshot| snapshot.dictionary_words.is_empty())
                            .unwrap_or(true),
                        entries,
                        loading: self.dashboard_loading,
                    },
                    &actions,
                    self.dictionary_search_input.as_ref(),
                    editor.as_ref(),
                    tokens,
                )
                .into_any_element()
            }
            DashboardSection::Snippets => {
                let sender = self.ui_action_tx.clone();
                let actions = SnippetsActionProps::new(move |action| {
                    let _ = sender.send(NativeUiAction::Snippets(action));
                });
                let entries = self
                    .filtered_snippet_entries()
                    .into_iter()
                    .map(|entry| SnippetEntryProps {
                        trigger: entry.trigger.clone().into(),
                        expansion: entry.expansion.clone().into(),
                        use_count: entry.use_count,
                    })
                    .collect::<Vec<_>>();
                let editor = match (
                    self.snippets_editor.as_ref(),
                    self.snippets_trigger_input.as_ref(),
                    self.snippets_expansion_input.as_ref(),
                ) {
                    (Some(target), Some(trigger), Some(expansion)) => Some(SnippetEditorInputs {
                        editing: matches!(target, SnippetEditorTarget::Existing(_)),
                        trigger: trigger.clone(),
                        expansion: expansion.clone(),
                    }),
                    _ => None,
                };
                snippets_view_with_actions(
                    &SnippetsProps {
                        active_tab: self.snippets_tab,
                        search_query: self.snippets_search.clone().into(),
                        show_intro: snapshot
                            .map(|snapshot| snapshot.snippets.is_empty())
                            .unwrap_or(true),
                        entries,
                        loading: self.dashboard_loading,
                    },
                    &actions,
                    self.snippets_search_input.as_ref(),
                    editor.as_ref(),
                    tokens,
                )
                .into_any_element()
            }
            DashboardSection::Style => {
                let sender = self.ui_action_tx.clone();
                let actions = StyleActionProps::new(move |action| {
                    let _ = sender.send(NativeUiAction::Style(action));
                });
                let tone = snapshot
                    .map(|snapshot| match self.style_context {
                        StyleContext::Personal => snapshot.dictation_style.personal,
                        StyleContext::Work => snapshot.dictation_style.work,
                        StyleContext::Email => snapshot.dictation_style.email,
                        StyleContext::Other => snapshot.dictation_style.other,
                    })
                    .map(Self::style_tone)
                    .unwrap_or(StyleTone::Formal);
                style_view_with_actions(
                    StyleProps {
                        context: self.style_context,
                        tone,
                    },
                    &actions,
                    tokens,
                )
                .into_any_element()
            }
        }
    }
}

impl Render for ListenOsApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = self.tokens;

        if !self.configuration_loaded {
            return div()
                .size_full()
                .bg(tokens.canvas)
                .text_color(tokens.text)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .child(div().text_lg().child("Starting ListenOS"))
                        .child(
                            div()
                                .text_sm()
                                .text_color(tokens.text_muted)
                                .child("Loading local models, microphones, and native settings…"),
                        ),
                )
                .into_any_element();
        }

        if !self.onboarding_complete {
            let mut onboarding_root = div()
                .size_full()
                .bg(tokens.canvas)
                .text_color(tokens.text)
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .child(self.onboarding.clone());
            if let Some(notice) = self.ui_notice.clone() {
                onboarding_root = onboarding_root.child(
                    div()
                        .absolute()
                        .top_4()
                        .left_4()
                        .right_4()
                        .rounded_md()
                        .border_1()
                        .border_color(tokens.warning)
                        .bg(tokens.muted)
                        .px_4()
                        .py_3()
                        .text_sm()
                        .text_color(tokens.text)
                        .child(notice),
                );
            }
            return onboarding_root.into_any_element();
        }

        match self.section {
            DashboardSection::Commands => self.ensure_command_inputs(window, cx),
            DashboardSection::Clipboard => self.ensure_clipboard_input(window, cx),
            DashboardSection::Dictionary => self.ensure_dictionary_inputs(window, cx),
            DashboardSection::Snippets => self.ensure_snippets_inputs(window, cx),
            _ => {}
        }

        let mut root = div()
            .relative()
            .size_full()
            .min_w(px(900.0))
            .min_h(px(620.0))
            .bg(tokens.canvas)
            .text_color(tokens.text)
            .flex()
            .child(
                div()
                    .w(px(220.0))
                    .h_full()
                    .bg(tokens.card)
                    .border_r_1()
                    .border_color(tokens.muted_border)
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_lg().child("ListenOS"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(tokens.text_muted)
                                    .child("Local voice workspace"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(DashboardSection::ALL.map(|item| self.nav_item(item, cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        ListenOsButton::new("open-native-settings", "Settings", tokens)
                            .variant(ListenOsButtonVariant::Secondary)
                            .accessibility_label("Open ListenOS settings")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings_open = true;
                                this.ui_notice = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .rounded_lg()
                            .border_1()
                            .border_color(tokens.border)
                            .bg(tokens.muted)
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(tokens.text_muted)
                                    .child("Runtime"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .child(self.runtime_ui.shortcut_status.clone()),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_3()
                            .child(self.content_card(
                                "Transcription",
                                format!(
                                    "{} · {}",
                                    self.runtime_ui.model, self.runtime_ui.transcription_phase
                                ),
                            ))
                            .child(self.content_card(
                                "Audio",
                                format!(
                                    "{} · {:>3}%",
                                    self.runtime_ui.audio_phase,
                                    (self.runtime_ui.audio_level * 100.0).round() as u32
                                ),
                            ))
                            .child(self.content_card("Delivery", self.overlay.label().to_string())),
                    )
                    .children(self.confirmation_card(cx))
                    .children(self.destructive_confirmation_card(cx))
                    .child(
                        div()
                            .id("dashboard-route-scroll")
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .pr_2()
                            .child(self.route_surface()),
                    )
                    .when_some(self.ui_notice.clone(), |content, notice| {
                        content.child(
                            div()
                                .rounded_md()
                                .border_1()
                                .border_color(tokens.warning)
                                .bg(tokens.muted)
                                .px_4()
                                .py_3()
                                .text_sm()
                                .text_color(tokens.text)
                                .child(notice),
                        )
                    }),
            );

        if self.settings_open {
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(tokens.overlay_scrim)
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_6()
                    .child(self.settings.clone()),
            );
        }

        root.into_any_element()
    }
}
