use crate::components::text_layout::{
    ONBOARDING_CHOICE_DETAIL, ONBOARDING_CHOICE_TITLE, muted_clamped, single_line_text,
};
use crate::theme::DesignTokens;
use gpui::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusTone {
    #[default]
    Neutral,
    Progress,
    Ready,
    Error,
}

impl StatusTone {
    fn color(self, tokens: DesignTokens) -> Hsla {
        match self {
            Self::Neutral => tokens.text_muted,
            Self::Progress => tokens.primary,
            Self::Ready => tokens.positive,
            Self::Error => tokens.negative,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ModelStatus {
    #[default]
    Unknown,
    Missing {
        model: SharedString,
    },
    Downloading {
        model: SharedString,
        progress_percent: Option<u8>,
        downloaded_bytes: Option<u64>,
        total_bytes: Option<u64>,
    },
    Loading {
        model: SharedString,
    },
    Ready {
        model: SharedString,
    },
    Error {
        model: Option<SharedString>,
        message: SharedString,
    },
}

impl ModelStatus {
    pub fn tone(&self) -> StatusTone {
        match self {
            Self::Unknown | Self::Missing { .. } => StatusTone::Neutral,
            Self::Downloading { .. } | Self::Loading { .. } => StatusTone::Progress,
            Self::Ready { .. } => StatusTone::Ready,
            Self::Error { .. } => StatusTone::Error,
        }
    }

    pub fn title(&self) -> SharedString {
        match self {
            Self::Unknown => "Checking local transcription".into(),
            Self::Missing { .. } => "Model download required".into(),
            Self::Downloading { .. } => "Downloading local model".into(),
            Self::Loading { .. } => "Loading local model".into(),
            Self::Ready { .. } => "Local transcription ready".into(),
            Self::Error { .. } => "Local transcription needs attention".into(),
        }
    }

    pub fn detail(&self) -> SharedString {
        match self {
            Self::Unknown => "Runtime status is not available yet.".into(),
            Self::Missing { model } => {
                format!("{model} must be downloaded before dictation can start.").into()
            }
            Self::Downloading {
                model,
                progress_percent,
                downloaded_bytes,
                total_bytes,
            } => {
                let transferred = match (downloaded_bytes, total_bytes) {
                    (Some(downloaded), Some(total)) if *total > 0 => Some(format!(
                        "{} / {}",
                        format_download_size(*downloaded),
                        format_download_size(*total)
                    )),
                    (Some(downloaded), _) if *downloaded > 0 => {
                        Some(format_download_size(*downloaded))
                    }
                    _ => None,
                };
                match (progress_percent, transferred) {
                    (Some(progress), Some(transferred)) => {
                        format!("Downloading {model} · {progress}% · {transferred}").into()
                    }
                    (Some(progress), None) => format!("Downloading {model} · {progress}%").into(),
                    (None, Some(transferred)) => {
                        format!("Downloading {model} · {transferred}").into()
                    }
                    (None, None) => format!("Downloading {model}…").into(),
                }
            }
            Self::Loading { model } => format!("Preparing {model} for local dictation.").into(),
            Self::Ready { model } => format!("Ready · {model}").into(),
            Self::Error { model, message } => match model {
                Some(model) => format!("{model} · {message}").into(),
                None => message.clone(),
            },
        }
    }
}

fn format_download_size(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    if bytes as f64 >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.1} MiB", bytes as f64 / MIB)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum MicrophoneStatus {
    #[default]
    Unknown,
    Unavailable {
        message: SharedString,
    },
    Selected {
        name: SharedString,
        is_default: bool,
    },
    Testing {
        name: SharedString,
        level: f32,
    },
    Ready {
        name: SharedString,
    },
    Error {
        name: Option<SharedString>,
        message: SharedString,
    },
}

impl MicrophoneStatus {
    pub fn tone(&self) -> StatusTone {
        match self {
            Self::Unknown | Self::Selected { .. } => StatusTone::Neutral,
            Self::Testing { .. } => StatusTone::Progress,
            Self::Ready { .. } => StatusTone::Ready,
            Self::Unavailable { .. } | Self::Error { .. } => StatusTone::Error,
        }
    }

    pub fn title(&self) -> SharedString {
        match self {
            Self::Unknown => "Checking microphone".into(),
            Self::Unavailable { .. } => "No supported microphone".into(),
            Self::Selected { .. } => "Microphone selected".into(),
            Self::Testing { .. } => "Testing microphone".into(),
            Self::Ready { .. } => "Microphone ready".into(),
            Self::Error { .. } => "Microphone needs attention".into(),
        }
    }

    pub fn detail(&self) -> SharedString {
        match self {
            Self::Unknown => "Microphone status is not available yet.".into(),
            Self::Unavailable { message } => message.clone(),
            Self::Selected { name, is_default } => {
                if *is_default {
                    format!("{name} · system default").into()
                } else {
                    name.clone()
                }
            }
            Self::Testing { name, level } => {
                let percent = (level.clamp(0.0, 1.0) * 100.0).round() as u8;
                format!("{name} · input {percent}%").into()
            }
            Self::Ready { name } => format!("Input detected · {name}").into(),
            Self::Error { name, message } => match name {
                Some(name) => format!("{name} · {message}").into(),
                None => message.clone(),
            },
        }
    }
}

pub fn status_panel(
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    tone: StatusTone,
    tokens: DesignTokens,
) -> impl IntoElement {
    let tone_color = tone.color(tokens);

    div()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(if tone == StatusTone::Error {
            tokens.negative
        } else {
            tokens.muted_border
        })
        .bg(tokens.muted)
        .p_3()
        .flex()
        .items_start()
        .gap_3()
        .child(div().mt_1().size_2().rounded_full().bg(tone_color))
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(single_line_text(
                    title,
                    ONBOARDING_CHOICE_TITLE,
                    tokens.text,
                ))
                .child(muted_clamped(detail, ONBOARDING_CHOICE_DETAIL, tokens)),
        )
}

pub fn model_status_panel(status: &ModelStatus, tokens: DesignTokens) -> impl IntoElement {
    status_panel(status.title(), status.detail(), status.tone(), tokens)
}

pub fn microphone_status_panel(
    status: &MicrophoneStatus,
    tokens: DesignTokens,
) -> impl IntoElement {
    status_panel(status.title(), status.detail(), status.tone(), tokens)
}

pub fn model_choice_card(
    label: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    selected: bool,
    downloaded: bool,
    tokens: DesignTokens,
) -> impl IntoElement {
    let marker = if downloaded { "Ready" } else { "Download" };

    div()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(if selected {
            tokens.primary
        } else {
            tokens.muted_border
        })
        .bg(if selected {
            tokens.accent
        } else {
            tokens.muted
        })
        .p_3()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(single_line_text(
                    label,
                    ONBOARDING_CHOICE_TITLE,
                    tokens.text,
                ))
                .child(muted_clamped(detail, ONBOARDING_CHOICE_DETAIL, tokens)),
        )
        .child(
            div()
                .px_2()
                .py_1()
                .rounded_full()
                .text_xs()
                .text_color(if downloaded {
                    tokens.positive
                } else {
                    tokens.text_muted
                })
                .child(marker),
        )
}

pub fn microphone_choice_card(
    name: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    selected: bool,
    blocked: bool,
    tokens: DesignTokens,
) -> impl IntoElement {
    div()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(if blocked {
            tokens.negative
        } else if selected {
            tokens.primary
        } else {
            tokens.muted_border
        })
        .bg(if selected {
            tokens.accent
        } else {
            tokens.muted
        })
        .p_3()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(single_line_text(name, ONBOARDING_CHOICE_TITLE, tokens.text))
                .child(muted_clamped(detail, ONBOARDING_CHOICE_DETAIL, tokens)),
        )
        .children(blocked.then(|| {
            div()
                .px_2()
                .py_1()
                .rounded_full()
                .text_xs()
                .text_color(tokens.negative)
                .child("Blocked")
        }))
}
