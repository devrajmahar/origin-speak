use crate::components::button::{ListenOsButton, ListenOsButtonVariant};
use crate::components::input::ListenOsInput;
use crate::components::status_presentation::{
    microphone_choice_card, microphone_status_panel, model_choice_card, model_status_panel,
};
use crate::components::switch::ListenOsSwitch;
use crate::components::text_layout::{SETTINGS_ROW_DESCRIPTION, muted_clamped};
use crate::theme::DesignTokens;
use crate::views::settings_support::{
    SETTINGS_CONTENT_PADDING, SETTINGS_MODAL_HEIGHT, SETTINGS_MODAL_WIDTH, SETTINGS_SIDEBAR_WIDTH,
    SettingsCategory, SettingsLanguageKind, SettingsSection, SettingsViewState,
    settings_category_label, settings_language_label, settings_language_options, settings_nav_row,
    settings_section_heading,
};
use gpui::prelude::{FluentBuilder as _, InteractiveElement as _, StatefulInteractiveElement as _};
use gpui::*;
use gpui_base::input::{InputEvent, InputState};
use std::sync::Arc;
use voice_os_lib::{VibeActivationMode, VibeCodingConfig, VibeFormattingStyle};

pub type SettingsActionCallback = Arc<dyn Fn(SettingsAction) + Send + Sync + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutKind {
    HoldToTalk,
    Assistant,
}

fn modifier_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "control"
            | "ctrl"
            | "alt"
            | "shift"
            | "platform"
            | "meta"
            | "super"
            | "command"
            | "cmd"
            | "function"
            | "fn"
    )
}

fn shortcut_key_label(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "space" => "Space".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        "enter" => "Enter".to_string(),
        "tab" => "Tab".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        other if other.len() == 1 => other.to_ascii_uppercase(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        }
    }
}

fn canonical_shortcut(keystroke: &Keystroke) -> Result<SharedString, SharedString> {
    if modifier_key(&keystroke.key) {
        return Err("Press a non-modifier key to finish the shortcut.".into());
    }
    if keystroke.modifiers.function {
        return Err("Fn shortcuts are not supported by the native global shortcut service.".into());
    }

    let modifiers = keystroke.modifiers;
    if !(modifiers.control || modifiers.alt || modifiers.shift || modifiers.platform) {
        return Err("Shortcut must include Ctrl, Alt, Shift, Cmd, or Meta.".into());
    }

    let mut parts = Vec::with_capacity(5);
    if modifiers.control {
        parts.push("Ctrl".to_string());
    }
    if modifiers.alt {
        parts.push("Alt".to_string());
    }
    if modifiers.shift {
        parts.push("Shift".to_string());
    }
    if modifiers.platform {
        parts.push("Meta".to_string());
    }
    parts.push(shortcut_key_label(&keystroke.key));
    Ok(parts.join("+").into())
}

#[derive(Clone, Debug, PartialEq)]
pub struct SettingsModelChoice {
    pub id: SharedString,
    pub label: SharedString,
    pub detail: SharedString,
    pub downloaded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SettingsMicrophoneChoice {
    pub name: SharedString,
    pub detail: SharedString,
    pub is_default: bool,
    pub blocked: bool,
}

#[derive(Clone, Debug)]
pub enum SettingsAction {
    CloseRequested,
    ChangeShortcut {
        kind: ShortcutKind,
        shortcut: SharedString,
    },
    ChangeLanguage {
        kind: SettingsLanguageKind,
        language: SharedString,
    },
    SelectMicrophone(SharedString),
    ToggleStartOnLogin(bool),
    ToggleShowInTray(bool),
    ToggleGpuAcceleration(bool),
    CheckForUpdates,
    SelectModel(SharedString),
    DownloadModel(SharedString),
    SetVibeCodingConfig(VibeCodingConfig),
}

#[derive(Clone)]
pub struct SettingsViewProps {
    pub app_version: SharedString,
    pub hold_to_talk_shortcut: SharedString,
    pub assistant_shortcut: SharedString,
    pub source_language: SharedString,
    pub target_language: SharedString,
    pub update_status: SharedString,
    pub update_checking: bool,
    pub start_on_login: bool,
    pub auto_start_supported: bool,
    pub show_in_tray: bool,
    pub tray_supported: bool,
    pub use_gpu: bool,
    pub gpu_status: SharedString,
    pub vibe_coding: VibeCodingConfig,
    pub models: Vec<SettingsModelChoice>,
    pub microphones: Vec<SettingsMicrophoneChoice>,
    pub on_action: Option<SettingsActionCallback>,
}

impl Default for SettingsViewProps {
    fn default() -> Self {
        Self {
            app_version: env!("CARGO_PKG_VERSION").into(),
            hold_to_talk_shortcut: if cfg!(target_os = "macos") {
                "Ctrl + Space".into()
            } else {
                "Meta + Ctrl + Space".into()
            },
            assistant_shortcut: "Ctrl + Alt + Space".into(),
            source_language: "Auto Detect".into(),
            target_language: "English".into(),
            update_status: "Check if a new version is available".into(),
            update_checking: false,
            start_on_login: false,
            auto_start_supported: true,
            show_in_tray: true,
            tray_supported: false,
            use_gpu: true,
            gpu_status: "GPU preferred when supported".into(),
            vibe_coding: VibeCodingConfig::default(),
            models: Vec::new(),
            microphones: Vec::new(),
            on_action: None,
        }
    }
}

pub struct SettingsView {
    tokens: DesignTokens,
    state: SettingsViewState,
    props: SettingsViewProps,
    shortcut_capture: Option<ShortcutKind>,
    shortcut_capture_error: Option<SharedString>,
    shortcut_capture_focus: Option<FocusHandle>,
    language_selector: Option<SettingsLanguageKind>,
    vibe_trigger_input: Option<Entity<InputState>>,
    vibe_trigger_subscription: Option<Subscription>,
    vibe_trigger_pending_value: Option<SharedString>,
}

impl SettingsView {
    pub fn new(state: SettingsViewState, props: SettingsViewProps) -> Self {
        Self {
            tokens: DesignTokens::default(),
            state,
            props,
            shortcut_capture: None,
            shortcut_capture_error: None,
            shortcut_capture_focus: None,
            language_selector: None,
            vibe_trigger_input: None,
            vibe_trigger_subscription: None,
            vibe_trigger_pending_value: None,
        }
    }

    pub fn state(&self) -> &SettingsViewState {
        &self.state
    }

    pub fn set_state(&mut self, state: SettingsViewState, cx: &mut Context<Self>) {
        self.state = state;
        cx.notify();
    }

    pub fn set_props(&mut self, props: SettingsViewProps, cx: &mut Context<Self>) {
        if self.props.vibe_coding.trigger_phrase != props.vibe_coding.trigger_phrase {
            self.vibe_trigger_pending_value = Some(props.vibe_coding.trigger_phrase.clone().into());
        }
        self.props = props;
        cx.notify();
    }

    fn ensure_vibe_trigger_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.vibe_trigger_input.is_none() {
            let initial = self.props.vibe_coding.trigger_phrase.clone();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(initial)
                    .placeholder("vibe")
            });
            let subscription =
                cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                    if !matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        return;
                    }

                    let value = input.read(cx).value();
                    let normalized = value.trim().to_lowercase();
                    let normalized = if normalized.is_empty() {
                        "vibe".to_string()
                    } else {
                        normalized
                    };
                    let mut config = this.props.vibe_coding.clone();
                    if config.trigger_phrase == normalized {
                        return;
                    }
                    config.trigger_phrase = normalized;
                    this.notify_action(SettingsAction::SetVibeCodingConfig(config));
                });
            self.vibe_trigger_input = Some(input);
            self.vibe_trigger_subscription = Some(subscription);
        }

        if let Some(value) = self.vibe_trigger_pending_value.take()
            && let Some(input) = &self.vibe_trigger_input
        {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }

        if let Some(input) = &self.vibe_trigger_input {
            input.update(cx, |state, cx| {
                state.set_disabled(
                    !self.props.vibe_coding.enabled
                        || self.props.vibe_coding.activation_mode
                            != VibeActivationMode::TriggerPhrase,
                    cx,
                )
            });
        }
    }

    fn notify_action(&self, action: SettingsAction) {
        if let Some(callback) = &self.props.on_action {
            callback(action);
        }
    }

    fn begin_shortcut_capture(
        &mut self,
        kind: ShortcutKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shortcut_capture = Some(kind);
        self.shortcut_capture_error = None;
        self.language_selector = None;
        if let Some(focus) = &self.shortcut_capture_focus {
            focus.focus(window, cx);
        }
        cx.notify();
    }

    fn handle_shortcut_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(kind) = self.shortcut_capture else {
            return;
        };

        cx.stop_propagation();
        if event.is_held {
            return;
        }

        if event.keystroke.key.eq_ignore_ascii_case("escape") {
            self.shortcut_capture = None;
            self.shortcut_capture_error = None;
            cx.notify();
            return;
        }

        match canonical_shortcut(&event.keystroke) {
            Ok(shortcut) => {
                self.shortcut_capture = None;
                self.shortcut_capture_error = None;
                self.notify_action(SettingsAction::ChangeShortcut { kind, shortcut });
            }
            Err(message) => {
                self.shortcut_capture_error = Some(message);
            }
        }
        cx.notify();
    }

    fn shortcut_capture_button(
        &self,
        id: &'static str,
        kind: ShortcutKind,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let capturing = self.shortcut_capture == Some(kind);
        let label = if capturing {
            "Press shortcut…"
        } else {
            "Change"
        };

        div()
            .flex_shrink_0()
            .child(
                ListenOsButton::new(id, label, self.tokens)
                    .variant(ListenOsButtonVariant::Secondary)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.begin_shortcut_capture(kind, window, cx);
                    })),
            )
            .into_any_element()
    }

    fn language_selector(
        &self,
        kind: SettingsLanguageKind,
        current: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.language_selector == Some(kind);
        let label = settings_language_label(kind, current.as_ref());
        let trigger_id = match kind {
            SettingsLanguageKind::Source => "source-language-selector",
            SettingsLanguageKind::Target => "target-language-selector",
        };
        let mut selector = div().w(px(180.0)).flex().flex_col().gap_1().child(
            ListenOsButton::new(trigger_id, label, self.tokens)
                .variant(ListenOsButtonVariant::Secondary)
                .accessibility_label(match kind {
                    SettingsLanguageKind::Source => "Select source language",
                    SettingsLanguageKind::Target => "Select target language",
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.language_selector = if this.language_selector == Some(kind) {
                        None
                    } else {
                        Some(kind)
                    };
                    this.shortcut_capture = None;
                    this.shortcut_capture_error = None;
                    cx.notify();
                })),
        );

        if open {
            selector = selector.child(
                div()
                    .id(format!("settings-language-options-{}", kind.id()))
                    .max_h(px(220.0))
                    .overflow_y_scroll()
                    .rounded_lg()
                    .border_1()
                    .border_color(self.tokens.border)
                    .bg(self.tokens.card)
                    .p_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(settings_language_options(kind).iter().enumerate().map(
                        |(index, option)| {
                            let code = SharedString::from(option.code);
                            let selected = option.code.eq_ignore_ascii_case(current.as_ref())
                                || option.label.eq_ignore_ascii_case(current.as_ref());
                            div()
                                .id(format!("settings-language-option-{}-{index}", kind.id()))
                                .child(
                                    ListenOsButton::new(
                                        format!("settings-language-{}-{}", kind.id(), option.code),
                                        if selected {
                                            format!("✓ {}", option.label)
                                        } else {
                                            option.label.to_string()
                                        },
                                        self.tokens,
                                    )
                                    .variant(ListenOsButtonVariant::Ghost)
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.language_selector = None;
                                            this.notify_action(SettingsAction::ChangeLanguage {
                                                kind,
                                                language: code.clone(),
                                            });
                                            cx.notify();
                                        },
                                    )),
                                )
                        },
                    )),
            );
        }

        selector.into_any_element()
    }

    fn nav_item(&self, section: SettingsSection, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.state.active_section == section;

        div()
            .id(format!("settings-section-{}", section.id()))
            .w_full()
            .cursor_pointer()
            .hover(|style| style.bg(self.tokens.accent))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.select(section);
                cx.notify();
            }))
            .child(settings_nav_row(section, selected, self.tokens))
            .into_any_element()
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings_items = SettingsSection::ALL
            .into_iter()
            .filter(|section| section.category() == SettingsCategory::Settings)
            .map(|section| self.nav_item(section, cx))
            .collect::<Vec<_>>();
        let account_items = SettingsSection::ALL
            .into_iter()
            .filter(|section| section.category() == SettingsCategory::Account)
            .map(|section| self.nav_item(section, cx))
            .collect::<Vec<_>>();

        div()
            .w(px(SETTINGS_SIDEBAR_WIDTH))
            .h_full()
            .flex_shrink_0()
            .bg(self.tokens.muted)
            .border_r_1()
            .border_color(self.tokens.muted_border)
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(settings_category_label(
                        SettingsCategory::Settings,
                        self.tokens,
                    ))
                    .child(div().flex().flex_col().gap_1().children(settings_items)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(settings_category_label(
                        SettingsCategory::Account,
                        self.tokens,
                    ))
                    .child(div().flex().flex_col().gap_1().children(account_items)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .px_3()
                    .text_xs()
                    .text_color(self.tokens.text_muted)
                    .child(format!("ListenOS v{}", self.props.app_version)),
            )
            .into_any_element()
    }

    fn action_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        action: SettingsAction,
        disabled: bool,
        _: &mut Context<Self>,
    ) -> AnyElement {
        let callback = self.props.on_action.clone();

        div()
            .flex_shrink_0()
            .child(
                ListenOsButton::new(id, label, self.tokens)
                    .variant(ListenOsButtonVariant::Secondary)
                    .disabled(disabled)
                    .on_click(move |_, _, _| {
                        if let Some(callback) = &callback {
                            callback(action.clone());
                        }
                    }),
            )
            .into_any_element()
    }

    fn settings_switch(
        &self,
        id: &'static str,
        checked: bool,
        enabled: bool,
        accessibility_label: &'static str,
        action: impl Fn(bool) -> SettingsAction + Send + Sync + 'static,
    ) -> AnyElement {
        let callback = self.props.on_action.clone();

        div()
            .flex_shrink_0()
            .child(
                ListenOsSwitch::new(id, checked, accessibility_label, self.tokens)
                    .disabled(!enabled)
                    .on_change(move |checked, _, _, _| {
                        if let Some(callback) = &callback {
                            callback(action(checked));
                        }
                    }),
            )
            .into_any_element()
    }

    fn settings_row(
        &self,
        label: impl Into<SharedString>,
        description: impl Into<SharedString>,
        action: Option<AnyElement>,
    ) -> AnyElement {
        div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_5()
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
                            .text_color(self.tokens.text)
                            .child(label.into()),
                    )
                    .child(muted_clamped(
                        description,
                        SETTINGS_ROW_DESCRIPTION,
                        self.tokens,
                    )),
            )
            .children(action)
            .into_any_element()
    }

    fn render_general(&self, cx: &mut Context<Self>) -> AnyElement {
        let shortcut_capture_notice = self.shortcut_capture.map(|kind| {
            let shortcut_name = match kind {
                ShortcutKind::HoldToTalk => "hold-to-talk",
                ShortcutKind::Assistant => "assistant",
            };
            div()
                .w_full()
                .rounded_lg()
                .border_1()
                .border_color(self.tokens.primary)
                .bg(self.tokens.accent)
                .p_3()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(self.tokens.text)
                        .child(format!("Press the new {shortcut_name} shortcut now")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(self.tokens.text_muted)
                        .child("Use at least one modifier. Press Escape to cancel."),
                )
                .children(self.shortcut_capture_error.as_ref().map(|message| {
                    div()
                        .text_xs()
                        .text_color(self.tokens.negative)
                        .child(message.clone())
                }))
        });
        let source_language_label = settings_language_label(
            SettingsLanguageKind::Source,
            self.props.source_language.as_ref(),
        );
        let target_language_label = settings_language_label(
            SettingsLanguageKind::Target,
            self.props.target_language.as_ref(),
        );
        let microphone_choices = self
            .props
            .microphones
            .iter()
            .cloned()
            .map(|microphone| {
                let selected = self.state.selected_microphone.as_ref() == Some(&microphone.name);
                let name = microphone.name.clone();
                let callback = self.props.on_action.clone();
                let blocked = microphone.blocked;

                div()
                    .id(format!("settings-microphone-{}", microphone.name))
                    .w_full()
                    .cursor_pointer()
                    .when(!blocked, |element| {
                        element.on_click(cx.listener(move |this, _, _, cx| {
                            this.state.selected_microphone = Some(name.clone());
                            if let Some(callback) = &callback {
                                callback(SettingsAction::SelectMicrophone(name.clone()));
                            }
                            cx.notify();
                        }))
                    })
                    .child(microphone_choice_card(
                        microphone.name,
                        microphone.detail,
                        selected,
                        blocked,
                        self.tokens,
                    ))
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let microphone_section = if microphone_choices.is_empty() {
            div().into_any_element()
        } else {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(self.tokens.text_muted)
                        .child("Available microphones"),
                )
                .children(microphone_choices)
                .into_any_element()
        };

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_6()
            .child(settings_section_heading(
                SettingsSection::General,
                self.tokens,
            ))
            .child(self.settings_row(
                "Hold-to-talk shortcut",
                format!("Hold {} and speak.", self.props.hold_to_talk_shortcut),
                Some(self.shortcut_capture_button(
                    "change-hold-to-talk-shortcut",
                    ShortcutKind::HoldToTalk,
                    cx,
                )),
            ))
            .child(self.settings_row(
                "Assistant mode shortcut",
                format!(
                    "Press {} to toggle idle assistant listening.",
                    self.props.assistant_shortcut
                ),
                Some(self.shortcut_capture_button(
                    "change-assistant-shortcut",
                    ShortcutKind::Assistant,
                    cx,
                )),
            ))
            .children(shortcut_capture_notice)
            .child(
                self.settings_row(
                    "Microphone",
                    self.state
                        .selected_microphone
                        .clone()
                        .unwrap_or_else(|| "Auto-detect (Default)".into()),
                    None,
                ),
            )
            .child(microphone_status_panel(
                &self.state.microphone_status,
                self.tokens,
            ))
            .child(microphone_section)
            .child(self.settings_row(
                "Source language",
                source_language_label,
                Some(self.language_selector(
                    SettingsLanguageKind::Source,
                    &self.props.source_language,
                    cx,
                )),
            ))
            .child(self.settings_row(
                "Target language",
                target_language_label,
                Some(self.language_selector(
                    SettingsLanguageKind::Target,
                    &self.props.target_language,
                    cx,
                )),
            ))
            .child(self.settings_row(
                "Check for updates",
                self.props.update_status.clone(),
                Some(self.action_button(
                    "check-for-updates",
                    if self.props.update_checking {
                        "Checking…"
                    } else {
                        "Check now"
                    },
                    SettingsAction::CheckForUpdates,
                    self.props.update_checking,
                    cx,
                )),
            ))
            .into_any_element()
    }

    fn render_system(&self, cx: &mut Context<Self>) -> AnyElement {
        let model_choices = self
            .props
            .models
            .iter()
            .cloned()
            .map(|model| {
                let selected = self.state.selected_model.as_ref() == Some(&model.id);
                let id = model.id.clone();
                let callback = self.props.on_action.clone();

                div()
                    .id(format!("settings-model-{}", model.id))
                    .w_full()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.selected_model = Some(id.clone());
                        if let Some(callback) = &callback {
                            callback(SettingsAction::SelectModel(id.clone()));
                        }
                        cx.notify();
                    }))
                    .child(model_choice_card(
                        model.label,
                        model.detail,
                        selected,
                        model.downloaded,
                        self.tokens,
                    ))
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let selected_model = self
            .state
            .selected_model
            .clone()
            .unwrap_or_else(|| "No model selected".into());
        let selected_model_info = self
            .props
            .models
            .iter()
            .find(|model| Some(&model.id) == self.state.selected_model.as_ref());
        let selected_model_downloading =
            match (&self.state.model_status, &self.state.selected_model) {
                (
                    crate::components::status_presentation::ModelStatus::Downloading {
                        model, ..
                    },
                    Some(selected),
                ) => model == selected,
                _ => false,
            };
        let download_label: SharedString = match &self.state.model_status {
            crate::components::status_presentation::ModelStatus::Downloading {
                model,
                progress_percent,
                ..
            } if self.state.selected_model.as_ref() == Some(model) => progress_percent
                .map(|progress| format!("Downloading {progress}%"))
                .unwrap_or_else(|| "Downloading…".to_string())
                .into(),
            _ => "Download".into(),
        };
        let download_action = selected_model_info
            .filter(|model| !model.downloaded)
            .map(|model| {
                self.action_button(
                    "download-selected-model",
                    download_label.clone(),
                    SettingsAction::DownloadModel(model.id.clone()),
                    selected_model_downloading,
                    cx,
                )
            });

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_6()
            .child(settings_section_heading(
                SettingsSection::System,
                self.tokens,
            ))
            .child(self.settings_row(
                "Start on login",
                "Automatically start ListenOS when you log in",
                Some(self.settings_switch(
                    "toggle-start-on-login",
                    self.props.start_on_login,
                    self.props.auto_start_supported,
                    "Start ListenOS on login",
                    SettingsAction::ToggleStartOnLogin,
                )),
            ))
            .child(self.settings_row(
                "Show in menu bar",
                "Display ListenOS icon in the system tray",
                Some(self.settings_switch(
                    "toggle-show-in-tray",
                    self.props.show_in_tray,
                    self.props.tray_supported,
                    "Show ListenOS in the system tray",
                    SettingsAction::ToggleShowInTray,
                )),
            ))
            .child(self.settings_row("Transcription model", selected_model, download_action))
            .child(self.settings_row(
                "Use GPU for transcription",
                self.props.gpu_status.clone(),
                Some(self.settings_switch(
                    "toggle-gpu-transcription",
                    self.props.use_gpu,
                    true,
                    "Use GPU for local transcription",
                    SettingsAction::ToggleGpuAcceleration,
                )),
            ))
            .child(
                div()
                    .id("settings-model-list")
                    .w_full()
                    .max_h(px(220.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(model_choices),
            )
            .child(model_status_panel(&self.state.model_status, self.tokens))
            .into_any_element()
    }

    fn vibe_choice_button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        selected: bool,
        next: VibeCodingConfig,
    ) -> AnyElement {
        let callback = self.props.on_action.clone();
        ListenOsButton::new(id, label, self.tokens)
            .variant(if selected {
                ListenOsButtonVariant::Primary
            } else {
                ListenOsButtonVariant::Secondary
            })
            .on_click(move |_, _, _| {
                if let Some(callback) = &callback {
                    callback(SettingsAction::SetVibeCodingConfig(next.clone()));
                }
            })
            .into_any_element()
    }

    fn render_vibe_coding(&self) -> AnyElement {
        let config = self.props.vibe_coding.clone();

        let enabled_config = {
            let mut next = config.clone();
            next.enabled = !config.enabled;
            next
        };

        let activation = [
            (
                "Automatic in coding contexts",
                VibeActivationMode::Automatic,
            ),
            (
                "Only with trigger phrase",
                VibeActivationMode::TriggerPhrase,
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (label, mode))| {
            let mut next = config.clone();
            next.activation_mode = mode;
            self.vibe_choice_button(
                ("vibe-activation", index),
                label,
                config.activation_mode == mode,
                next,
            )
        })
        .collect::<Vec<_>>();

        let formatting = [
            ("Natural", VibeFormattingStyle::Natural),
            ("Structured", VibeFormattingStyle::Structured),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (label, formatting))| {
            let mut next = config.clone();
            next.formatting = formatting;
            self.vibe_choice_button(
                ("vibe-formatting", index),
                label,
                config.formatting == formatting,
                next,
            )
        })
        .collect::<Vec<_>>();

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_6()
            .child(settings_section_heading(
                SettingsSection::VibeCoding,
                self.tokens,
            ))
            .child(self.settings_row(
                "Enable vibe coding",
                "Clean up dictated coding instructions without adding requirements.",
                Some(self.settings_switch(
                    "toggle-vibe-coding",
                    config.enabled,
                    true,
                    "Enable vibe coding",
                    move |_| SettingsAction::SetVibeCodingConfig(enabled_config.clone()),
                )),
            ))
            .child(self.settings_row(
                "Activation",
                "Choose when ListenOS formats dictated coding instructions.",
                Some(div().flex().flex_wrap().gap_2().children(activation).into_any_element()),
            ))
            .child(self.settings_row(
                "Trigger phrase",
                format!("Used only in trigger-phrase mode: “{}”", config.trigger_phrase),
                self.vibe_trigger_input.as_ref().map(|input| {
                    ListenOsInput::new(input, self.tokens)
                        .width(px(180.0))
                        .into_any_element()
                }),
            ))
            .child(self.settings_row(
                "Formatting",
                "Natural keeps prose compact; Structured groups the same instructions for scanning.",
                Some(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(formatting)
                        .into_any_element(),
                ),
            ))
            .child(
                div()
                    .w_full()
                    .rounded_lg()
                    .border_1()
                    .border_color(self.tokens.muted_border)
                    .bg(self.tokens.muted)
                    .p_4()
                    .text_sm()
                    .text_color(self.tokens.text_muted)
                    .child("Vibe coding only cleans and organizes what you dictated. It does not choose a tool, architecture, tests, constraints, or implementation details for the downstream coding assistant."),
            )
            .into_any_element()
    }

    fn render_experimental(&self) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(settings_section_heading(
                SettingsSection::Experimental,
                self.tokens,
            ))
            .child(
                div()
                    .rounded_lg()
                    .border_1()
                    .border_color(self.tokens.muted_border)
                    .bg(self.tokens.muted)
                    .p_4()
                    .text_sm()
                    .text_color(self.tokens.text_muted)
                    .child("No experimental features are available at this time."),
            )
            .into_any_element()
    }

    fn render_account(&self) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_6()
            .child(settings_section_heading(
                SettingsSection::Account,
                self.tokens,
            ))
            .child(
                self.settings_row(
                    "Mode",
                    "Self-hosted local mode. No sign-in required.",
                    Some(
                        div()
                            .text_sm()
                            .text_color(self.tokens.positive)
                            .child("Local only")
                            .into_any_element(),
                    ),
                ),
            )
            .child(
                self.settings_row(
                    "Profile",
                    "Local user",
                    Some(
                        div()
                            .text_sm()
                            .text_color(self.tokens.text_muted)
                            .child("Stored on this device")
                            .into_any_element(),
                    ),
                ),
            )
            .into_any_element()
    }

    fn render_local_info(&self, section: SettingsSection, message: &'static str) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(settings_section_heading(section, self.tokens))
            .child(
                div()
                    .rounded_lg()
                    .border_1()
                    .border_color(self.tokens.muted_border)
                    .bg(self.tokens.muted)
                    .p_4()
                    .text_sm()
                    .text_color(self.tokens.text_muted)
                    .child(message),
            )
            .into_any_element()
    }

    fn render_privacy(&self) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_6()
            .child(settings_section_heading(
                SettingsSection::Privacy,
                self.tokens,
            ))
            .child(
                self.settings_row(
                    "Voice data",
                    "Voice recordings are processed locally and are not retained after processing.",
                    Some(
                        div()
                            .text_sm()
                            .text_color(self.tokens.positive)
                            .child("Local")
                            .into_any_element(),
                    ),
                ),
            )
            .child(
                self.settings_row(
                    "Speech models",
                    "Whisper model files are stored on this device and run locally.",
                    Some(
                        div()
                            .text_sm()
                            .text_color(self.tokens.text_muted)
                            .child("On device")
                            .into_any_element(),
                    ),
                ),
            )
            .into_any_element()
    }

    fn section_content(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.state.active_section {
            SettingsSection::General => self.render_general(cx),
            SettingsSection::System => self.render_system(cx),
            SettingsSection::VibeCoding => self.render_vibe_coding(),
            SettingsSection::Experimental => self.render_experimental(),
            SettingsSection::Account => self.render_account(),
            SettingsSection::Team => self.render_local_info(
                SettingsSection::Team,
                "Team sync is disabled in self-hosted local mode.",
            ),
            SettingsSection::Billing => self.render_local_info(
                SettingsSection::Billing,
                "ListenOS local mode has no account billing. Speech models are stored and run on this device.",
            ),
            SettingsSection::Privacy => self.render_privacy(),
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_vibe_trigger_input(window, cx);
        let shortcut_capture_focus = self
            .shortcut_capture_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let callback = self.props.on_action.clone();
        let close_button = ListenOsButton::new("close-settings", "×", self.tokens)
            .variant(ListenOsButtonVariant::Ghost)
            .accessibility_label("Close settings")
            .on_click(move |_, _, _| {
                if let Some(callback) = &callback {
                    callback(SettingsAction::CloseRequested);
                }
            });

        div()
            .track_focus(&shortcut_capture_focus)
            .on_key_down(cx.listener(|this, event, _, cx| {
                this.handle_shortcut_key(event, cx);
            }))
            .w(px(SETTINGS_MODAL_WIDTH))
            .h(px(SETTINGS_MODAL_HEIGHT))
            .rounded_lg()
            .border_1()
            .border_color(self.tokens.border)
            .bg(self.tokens.card)
            .text_color(self.tokens.text)
            .flex()
            .overflow_hidden()
            .child(self.sidebar(cx))
            .child(
                div()
                    .id("settings-content-scroll")
                    .relative()
                    .flex_1()
                    .h_full()
                    .min_w(px(0.0))
                    .overflow_y_scroll()
                    .p(px(SETTINGS_CONTENT_PADDING))
                    .child(div().absolute().top_4().right_4().child(close_button))
                    .child(self.section_content(cx)),
            )
    }
}
