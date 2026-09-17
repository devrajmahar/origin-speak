use crate::components::status_presentation::{MicrophoneStatus, ModelStatus};
use crate::components::text_layout::{ONBOARDING_BODY, muted_clamped};
use crate::theme::DesignTokens;
use gpui::*;

pub const ONBOARDING_MODAL_MAX_WIDTH: f32 = 448.0;
pub const ONBOARDING_MODAL_PADDING: f32 = 24.0;
pub const ONBOARDING_MIC_LIST_MAX_HEIGHT: f32 = 192.0;
pub const ONBOARDING_MODEL_LIST_MAX_HEIGHT: f32 = 208.0;
pub const ONBOARDING_COMMAND_LIST_MAX_HEIGHT: f32 = 192.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnboardingStep {
    #[default]
    Welcome,
    Model,
    Microphone,
    Test,
    Commands,
    Complete,
}

impl OnboardingStep {
    pub const ALL: [Self; 6] = [
        Self::Welcome,
        Self::Model,
        Self::Microphone,
        Self::Test,
        Self::Commands,
        Self::Complete,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::Welcome => 0,
            Self::Model => 1,
            Self::Microphone => 2,
            Self::Test => 3,
            Self::Commands => 4,
            Self::Complete => 5,
        }
    }

    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self.index() + 1).copied()
    }

    pub fn previous(self) -> Option<Self> {
        self.index()
            .checked_sub(1)
            .and_then(|index| Self::ALL.get(index).copied())
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome to ListenOS",
            Self::Model => "Set up local transcription",
            Self::Microphone => "Select Your Microphone",
            Self::Test => "Test Your Microphone",
            Self::Commands => "Quick Start Commands",
            Self::Complete => "You're All Set!",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Welcome => "Let's set up a few things to get you started.",
            Self::Model => {
                "Choose a speech model. ListenOS keeps dictation on this device and downloads the model if needed."
            }
            Self::Microphone => {
                "Choose a microphone for voice commands. Hands-free Bluetooth inputs are blocked to prevent audio hijacking."
            }
            Self::Test => "Let's make sure your microphone is working.",
            Self::Commands => "Select some command templates to get started.",
            Self::Complete => "ListenOS is ready to use.",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OnboardingViewState {
    pub step: OnboardingStep,
    pub selected_model: Option<SharedString>,
    pub selected_microphone: Option<SharedString>,
    pub selected_templates: Vec<SharedString>,
    pub model_status: ModelStatus,
    pub microphone_status: MicrophoneStatus,
}

impl OnboardingViewState {
    pub fn advance(&mut self) -> bool {
        if let Some(next) = self.step.next() {
            self.step = next;
            true
        } else {
            false
        }
    }

    pub fn go_back(&mut self) -> bool {
        if let Some(previous) = self.step.previous() {
            self.step = previous;
            true
        } else {
            false
        }
    }
}

pub fn onboarding_progress(step: OnboardingStep, tokens: DesignTokens) -> impl IntoElement {
    let current = step.index();

    div()
        .w_full()
        .flex()
        .gap_1()
        .children(
            OnboardingStep::ALL
                .into_iter()
                .enumerate()
                .map(|(index, _)| {
                    div()
                        .flex_1()
                        .h(px(4.0))
                        .rounded_full()
                        .bg(if index <= current {
                            tokens.primary
                        } else {
                            tokens.muted_border
                        })
                }),
        )
}

pub fn onboarding_heading(step: OnboardingStep, tokens: DesignTokens) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .max_w(px(400.0))
                .line_clamp(2)
                .text_xl()
                .text_color(tokens.text)
                .child(step.title()),
        )
        .child(muted_clamped(step.description(), ONBOARDING_BODY, tokens))
}
