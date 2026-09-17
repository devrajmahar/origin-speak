use crate::components::button::{ListenOsButton, ListenOsButtonVariant};
use crate::components::status_presentation::{
    microphone_choice_card, microphone_status_panel, model_choice_card, model_status_panel,
};
use crate::components::text_layout::{
    ONBOARDING_CHOICE_DETAIL, ONBOARDING_CHOICE_TITLE, muted_clamped, single_line_text,
};
use crate::theme::DesignTokens;
use crate::views::onboarding_support::{
    ONBOARDING_COMMAND_LIST_MAX_HEIGHT, ONBOARDING_MIC_LIST_MAX_HEIGHT, ONBOARDING_MODAL_MAX_WIDTH,
    ONBOARDING_MODAL_PADDING, ONBOARDING_MODEL_LIST_MAX_HEIGHT, OnboardingStep,
    OnboardingViewState, onboarding_heading, onboarding_progress,
};
use gpui::prelude::{FluentBuilder as _, InteractiveElement as _, StatefulInteractiveElement as _};
use gpui::*;
use std::sync::Arc;

pub type OnboardingActionCallback = Arc<dyn Fn(OnboardingAction) + Send + Sync + 'static>;

#[derive(Clone, Debug, PartialEq)]
pub struct OnboardingModelChoice {
    pub id: SharedString,
    pub label: SharedString,
    pub detail: SharedString,
    pub downloaded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OnboardingMicrophoneChoice {
    pub name: SharedString,
    pub detail: SharedString,
    pub is_default: bool,
    pub blocked: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OnboardingCommandChoice {
    pub id: SharedString,
    pub name: SharedString,
    pub trigger_phrase: SharedString,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OnboardingAction {
    StepChanged(OnboardingStep),
    ModelSelected(SharedString),
    ModelContinueRequested(SharedString),
    DownloadModelRequested(SharedString),
    MicrophoneSelected(SharedString),
    TestMicrophoneRequested,
    CommandTemplateToggled { id: SharedString, selected: bool },
    CompleteRequested,
}

#[derive(Clone)]
pub struct OnboardingViewProps {
    pub hold_to_talk_shortcut: SharedString,
    pub models: Vec<OnboardingModelChoice>,
    pub microphones: Vec<OnboardingMicrophoneChoice>,
    pub command_templates: Vec<OnboardingCommandChoice>,
    pub on_action: Option<OnboardingActionCallback>,
}

impl Default for OnboardingViewProps {
    fn default() -> Self {
        Self {
            hold_to_talk_shortcut: if cfg!(target_os = "macos") {
                "Ctrl + Space".into()
            } else {
                "Meta + Ctrl + Space".into()
            },
            models: Vec::new(),
            microphones: Vec::new(),
            command_templates: Vec::new(),
            on_action: None,
        }
    }
}

pub struct OnboardingView {
    tokens: DesignTokens,
    state: OnboardingViewState,
    props: OnboardingViewProps,
}

impl OnboardingView {
    pub fn new(state: OnboardingViewState, props: OnboardingViewProps) -> Self {
        Self {
            tokens: DesignTokens::default(),
            state,
            props,
        }
    }

    pub fn state(&self) -> &OnboardingViewState {
        &self.state
    }

    pub fn set_state(&mut self, state: OnboardingViewState, cx: &mut Context<Self>) {
        self.state = state;
        cx.notify();
    }

    pub fn set_props(&mut self, props: OnboardingViewProps, cx: &mut Context<Self>) {
        self.props = props;
        cx.notify();
    }

    fn notify_action(&self, action: OnboardingAction) {
        if let Some(callback) = &self.props.on_action {
            callback(action);
        }
    }

    fn primary_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = ListenOsButton::new(id, label, self.tokens)
            .disabled(!enabled)
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)));

        div().flex_1().child(button).into_any_element()
    }

    fn secondary_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = ListenOsButton::new(id, label, self.tokens)
            .variant(ListenOsButtonVariant::Secondary)
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)));

        div().flex_1().child(button).into_any_element()
    }

    fn advance(&mut self, cx: &mut Context<Self>) {
        if self.state.advance() {
            self.notify_action(OnboardingAction::StepChanged(self.state.step));
            cx.notify();
        }
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.state.go_back() {
            self.notify_action(OnboardingAction::StepChanged(self.state.step));
            cx.notify();
        }
    }

    fn navigation(
        &self,
        primary_label: &'static str,
        primary_enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .w_full()
            .flex()
            .gap_2()
            .child(self.secondary_button(
                "onboarding-back",
                "Back",
                |this, cx| this.go_back(cx),
                cx,
            ))
            .child(self.primary_button(
                "onboarding-next",
                primary_label,
                primary_enabled,
                |this, cx| this.advance(cx),
                cx,
            ))
            .into_any_element()
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_5()
            .child(
                div()
                    .size(px(56.0))
                    .rounded_lg()
                    .bg(self.tokens.accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xl()
                    .text_color(self.tokens.primary)
                    .child("L"),
            )
            .child(onboarding_heading(OnboardingStep::Welcome, self.tokens))
            .child(self.primary_button(
                "onboarding-get-started",
                "Get Started",
                true,
                |this, cx| this.advance(cx),
                cx,
            ))
            .into_any_element()
    }

    fn render_model(&self, cx: &mut Context<Self>) -> AnyElement {
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
                    .id(format!("onboarding-model-{}", model.id))
                    .w_full()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.selected_model = Some(id.clone());
                        if let Some(callback) = &callback {
                            callback(OnboardingAction::ModelSelected(id.clone()));
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

        let selected_model_info = self
            .props
            .models
            .iter()
            .find(|model| Some(&model.id) == self.state.selected_model.as_ref())
            .cloned();
        let selected_model_downloaded = selected_model_info
            .as_ref()
            .map(|model| model.downloaded)
            .unwrap_or(false);
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
        let primary_label: SharedString = match &self.state.model_status {
            crate::components::status_presentation::ModelStatus::Downloading {
                model,
                progress_percent,
                ..
            } if self.state.selected_model.as_ref() == Some(model) => progress_percent
                .map(|progress| format!("Downloading {progress}%"))
                .unwrap_or_else(|| "Downloading…".to_string())
                .into(),
            _ if selected_model_downloaded => "Continue".into(),
            _ => "Download & continue".into(),
        };
        let callback = self.props.on_action.clone();
        let selected_model_for_action = selected_model_info.clone();

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(onboarding_heading(OnboardingStep::Model, self.tokens))
            .child(
                div()
                    .id("onboarding-model-list")
                    .w_full()
                    .max_h(px(ONBOARDING_MODEL_LIST_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(model_choices),
            )
            .child(model_status_panel(&self.state.model_status, self.tokens))
            .child(
                div()
                    .w_full()
                    .flex()
                    .gap_2()
                    .child(self.secondary_button(
                        "onboarding-back",
                        "Back",
                        |this, cx| this.go_back(cx),
                        cx,
                    ))
                    .child(self.primary_button(
                        "onboarding-model-next",
                        primary_label,
                        selected_model_info.is_some() && !selected_model_downloading,
                        move |_, _| {
                            let Some(model) = &selected_model_for_action else {
                                return;
                            };
                            if let Some(callback) = &callback {
                                if model.downloaded {
                                    callback(OnboardingAction::ModelContinueRequested(
                                        model.id.clone(),
                                    ));
                                } else {
                                    callback(OnboardingAction::DownloadModelRequested(
                                        model.id.clone(),
                                    ));
                                }
                            }
                        },
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn render_microphone(&self, cx: &mut Context<Self>) -> AnyElement {
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
                    .id(format!("onboarding-microphone-{}", microphone.name))
                    .w_full()
                    .when(!blocked, |element| {
                        element
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.selected_microphone = Some(name.clone());
                                if let Some(callback) = &callback {
                                    callback(OnboardingAction::MicrophoneSelected(name.clone()));
                                }
                                cx.notify();
                            }))
                    })
                    .child(microphone_choice_card(
                        microphone.name,
                        if microphone.is_default {
                            "System default".into()
                        } else {
                            microphone.detail
                        },
                        selected,
                        blocked,
                        self.tokens,
                    ))
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(onboarding_heading(OnboardingStep::Microphone, self.tokens))
            .child(
                div()
                    .id("onboarding-microphone-list")
                    .w_full()
                    .max_h(px(ONBOARDING_MIC_LIST_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(microphone_choices),
            )
            .child(microphone_status_panel(
                &self.state.microphone_status,
                self.tokens,
            ))
            .child(self.navigation("Continue", self.state.selected_microphone.is_some(), cx))
            .into_any_element()
    }

    fn render_test(&self, cx: &mut Context<Self>) -> AnyElement {
        let (button_label, detail) = match &self.state.microphone_status {
            crate::components::status_presentation::MicrophoneStatus::Testing { .. } => (
                "Listening…",
                "Speak normally while ListenOS checks the input.",
            ),
            crate::components::status_presentation::MicrophoneStatus::Ready { .. } => {
                ("Test again", "Microphone input detected.")
            }
            _ => (
                "Test microphone",
                "Run a short local microphone input test.",
            ),
        };
        let callback = self.props.on_action.clone();
        let microphone_ready = matches!(
            self.state.microphone_status,
            crate::components::status_presentation::MicrophoneStatus::Ready { .. }
        );

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(onboarding_heading(OnboardingStep::Test, self.tokens))
            .child(microphone_status_panel(
                &self.state.microphone_status,
                self.tokens,
            ))
            .child(
                div()
                    .id("test-microphone")
                    .w_full()
                    .rounded_lg()
                    .border_1()
                    .border_color(self.tokens.muted_border)
                    .bg(self.tokens.muted)
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
                            .child(single_line_text(
                                button_label,
                                ONBOARDING_CHOICE_TITLE,
                                self.tokens.text,
                            ))
                            .child(muted_clamped(detail, ONBOARDING_CHOICE_DETAIL, self.tokens)),
                    )
                    .child(
                        ListenOsButton::new("run-microphone-test", button_label, self.tokens)
                            .on_click(move |_, _, _| {
                                if let Some(callback) = &callback {
                                    callback(OnboardingAction::TestMicrophoneRequested);
                                }
                            }),
                    ),
            )
            .child(self.navigation("Continue", microphone_ready, cx))
            .into_any_element()
    }

    fn render_commands(&self, cx: &mut Context<Self>) -> AnyElement {
        let command_choices = self
            .props
            .command_templates
            .iter()
            .cloned()
            .map(|command| {
                let selected = self.state.selected_templates.contains(&command.id);
                let id = command.id.clone();
                let callback = self.props.on_action.clone();

                div()
                    .id(format!("onboarding-command-{}", command.id))
                    .w_full()
                    .rounded_lg()
                    .border_1()
                    .border_color(if selected {
                        self.tokens.primary
                    } else {
                        self.tokens.muted_border
                    })
                    .bg(if selected {
                        self.tokens.accent
                    } else {
                        self.tokens.muted
                    })
                    .p_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .hover(|style| style.bg(self.tokens.accent))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let was_selected = this.state.selected_templates.contains(&id);
                        if was_selected {
                            this.state.selected_templates.retain(|item| item != &id);
                        } else {
                            this.state.selected_templates.push(id.clone());
                        }
                        if let Some(callback) = &callback {
                            callback(OnboardingAction::CommandTemplateToggled {
                                id: id.clone(),
                                selected: !was_selected,
                            });
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(single_line_text(
                                command.name,
                                ONBOARDING_CHOICE_TITLE,
                                self.tokens.text,
                            ))
                            .child(muted_clamped(
                                format!("“{}”", command.trigger_phrase),
                                ONBOARDING_CHOICE_DETAIL,
                                self.tokens,
                            )),
                    )
                    .child(
                        div()
                            .size_4()
                            .rounded_sm()
                            .border_1()
                            .border_color(if selected {
                                self.tokens.primary
                            } else {
                                self.tokens.border
                            })
                            .bg(if selected {
                                self.tokens.primary
                            } else {
                                self.tokens.card
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .text_color(self.tokens.primary_foreground)
                            .child(if selected { "✓" } else { "" }),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(onboarding_heading(OnboardingStep::Commands, self.tokens))
            .child(
                div()
                    .id("onboarding-command-list")
                    .w_full()
                    .max_h(px(ONBOARDING_COMMAND_LIST_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(command_choices),
            )
            .child(self.navigation("Continue", true, cx))
            .into_any_element()
    }

    fn render_complete(&self, cx: &mut Context<Self>) -> AnyElement {
        let callback = self.props.on_action.clone();

        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_5()
            .child(
                div()
                    .size(px(56.0))
                    .rounded_lg()
                    .bg(self.tokens.muted)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_2xl()
                    .text_color(self.tokens.positive)
                    .child("✓"),
            )
            .child(onboarding_heading(OnboardingStep::Complete, self.tokens))
            .child(
                div()
                    .w_full()
                    .rounded_lg()
                    .bg(self.tokens.muted)
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_sm().text_color(self.tokens.text).child(format!(
                        "{} · Hold to speak",
                        self.props.hold_to_talk_shortcut
                    )))
                    .child(
                        div().text_xs().text_color(self.tokens.text_muted).child(
                            "Release when done. ListenOS will process your command locally.",
                        ),
                    ),
            )
            .child(self.primary_button(
                "onboarding-complete",
                "Start Using ListenOS",
                true,
                move |_, _| {
                    if let Some(callback) = &callback {
                        callback(OnboardingAction::CompleteRequested);
                    }
                },
                cx,
            ))
            .into_any_element()
    }

    fn step_content(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.state.step {
            OnboardingStep::Welcome => self.render_welcome(cx),
            OnboardingStep::Model => self.render_model(cx),
            OnboardingStep::Microphone => self.render_microphone(cx),
            OnboardingStep::Test => self.render_test(cx),
            OnboardingStep::Commands => self.render_commands(cx),
            OnboardingStep::Complete => self.render_complete(cx),
        }
    }
}

impl Render for OnboardingView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .max_w(px(ONBOARDING_MODAL_MAX_WIDTH))
            .rounded_lg()
            .border_1()
            .border_color(self.tokens.border)
            .bg(self.tokens.card)
            .text_color(self.tokens.text)
            .p(px(ONBOARDING_MODAL_PADDING))
            .flex()
            .flex_col()
            .gap_6()
            .child(onboarding_progress(self.state.step, self.tokens))
            .child(self.step_content(cx))
    }
}
