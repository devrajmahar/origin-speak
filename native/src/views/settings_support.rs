use crate::components::status_presentation::{MicrophoneStatus, ModelStatus};
use crate::components::text_layout::{SETTINGS_NAV_LABEL, single_line_text};
use crate::theme::DesignTokens;
use gpui::*;

pub const SETTINGS_MODAL_WIDTH: f32 = 800.0;
pub const SETTINGS_MODAL_HEIGHT: f32 = 600.0;
pub const SETTINGS_SIDEBAR_WIDTH: f32 = 224.0;
pub const SETTINGS_CONTENT_PADDING: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsLanguageKind {
    Source,
    Target,
}

impl SettingsLanguageKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Target => "target",
        }
    }

    pub fn allows_auto(self) -> bool {
        self == Self::Source
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsLanguageOption {
    pub code: &'static str,
    pub label: &'static str,
}

const LANGUAGE_OPTIONS: [SettingsLanguageOption; 13] = [
    SettingsLanguageOption {
        code: "auto",
        label: "Auto Detect",
    },
    SettingsLanguageOption {
        code: "en",
        label: "English",
    },
    SettingsLanguageOption {
        code: "hi",
        label: "Hindi",
    },
    SettingsLanguageOption {
        code: "es",
        label: "Spanish",
    },
    SettingsLanguageOption {
        code: "fr",
        label: "French",
    },
    SettingsLanguageOption {
        code: "de",
        label: "German",
    },
    SettingsLanguageOption {
        code: "it",
        label: "Italian",
    },
    SettingsLanguageOption {
        code: "pt",
        label: "Portuguese",
    },
    SettingsLanguageOption {
        code: "ru",
        label: "Russian",
    },
    SettingsLanguageOption {
        code: "zh",
        label: "Chinese",
    },
    SettingsLanguageOption {
        code: "ja",
        label: "Japanese",
    },
    SettingsLanguageOption {
        code: "ko",
        label: "Korean",
    },
    SettingsLanguageOption {
        code: "ar",
        label: "Arabic",
    },
];

pub fn settings_language_options(kind: SettingsLanguageKind) -> &'static [SettingsLanguageOption] {
    if kind.allows_auto() {
        &LANGUAGE_OPTIONS
    } else {
        &LANGUAGE_OPTIONS[1..]
    }
}

pub fn settings_language_label(kind: SettingsLanguageKind, value: &str) -> SharedString {
    settings_language_options(kind)
        .iter()
        .find(|option| {
            option.code.eq_ignore_ascii_case(value) || option.label.eq_ignore_ascii_case(value)
        })
        .map(|option| SharedString::from(option.label))
        .unwrap_or_else(|| SharedString::from(value.to_string()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsCategory {
    Settings,
    Account,
}

impl SettingsCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::Account => "Account",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsSection {
    #[default]
    General,
    System,
    VibeCoding,
    Experimental,
    Account,
    Team,
    Billing,
    Privacy,
}

impl SettingsSection {
    pub const ALL: [Self; 8] = [
        Self::General,
        Self::System,
        Self::VibeCoding,
        Self::Experimental,
        Self::Account,
        Self::Team,
        Self::Billing,
        Self::Privacy,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::System => "system",
            Self::VibeCoding => "vibe-coding",
            Self::Experimental => "experimental",
            Self::Account => "account",
            Self::Team => "team",
            Self::Billing => "billing",
            Self::Privacy => "privacy",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::System => "System",
            Self::VibeCoding => "Vibe coding",
            Self::Experimental => "Experimental",
            Self::Account => "Account",
            Self::Team => "Team",
            Self::Billing => "Plans and Billing",
            Self::Privacy => "Data and Privacy",
        }
    }

    pub fn category(self) -> SettingsCategory {
        match self {
            Self::General | Self::System | Self::VibeCoding | Self::Experimental => {
                SettingsCategory::Settings
            }
            Self::Account | Self::Team | Self::Billing | Self::Privacy => SettingsCategory::Account,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsViewState {
    pub active_section: SettingsSection,
    pub selected_model: Option<SharedString>,
    pub selected_microphone: Option<SharedString>,
    pub model_status: ModelStatus,
    pub microphone_status: MicrophoneStatus,
}

impl SettingsViewState {
    pub fn select(&mut self, section: SettingsSection) {
        self.active_section = section;
    }
}

/// Non-interactive nav presentation. The owning view can attach GPUI listeners
/// around this element without duplicating the visual contract.
pub fn settings_nav_row(
    section: SettingsSection,
    selected: bool,
    tokens: DesignTokens,
) -> impl IntoElement {
    div()
        .w_full()
        .rounded_lg()
        .px_3()
        .py_2()
        .bg(if selected {
            tokens.accent
        } else {
            crate::theme::transparent()
        })
        .text_color(if selected {
            tokens.text
        } else {
            tokens.text_muted
        })
        .child(single_line_text(
            section.label(),
            SETTINGS_NAV_LABEL,
            if selected {
                tokens.text
            } else {
                tokens.text_muted
            },
        ))
}

pub fn settings_category_label(
    category: SettingsCategory,
    tokens: DesignTokens,
) -> impl IntoElement {
    div()
        .px_3()
        .text_xs()
        .text_color(tokens.text_muted)
        .child(category.label())
}

pub fn settings_section_heading(
    section: SettingsSection,
    tokens: DesignTokens,
) -> impl IntoElement {
    div()
        .max_w(px(420.0))
        .line_clamp(2)
        .text_2xl()
        .text_color(tokens.text)
        .child(section.label())
}
