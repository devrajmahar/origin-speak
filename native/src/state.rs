#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DashboardSection {
    Dashboard,
    Conversation,
    Commands,
    Clipboard,
    Integrations,
    Dictionary,
    Snippets,
    Style,
}

impl DashboardSection {
    pub const ALL: [Self; 8] = [
        Self::Dashboard,
        Self::Conversation,
        Self::Commands,
        Self::Clipboard,
        Self::Integrations,
        Self::Dictionary,
        Self::Snippets,
        Self::Style,
    ];

    pub fn id(self) -> usize {
        match self {
            Self::Dashboard => 0,
            Self::Conversation => 1,
            Self::Commands => 2,
            Self::Clipboard => 3,
            Self::Integrations => 4,
            Self::Dictionary => 5,
            Self::Snippets => 6,
            Self::Style => 7,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Conversation => "Conversation",
            Self::Commands => "Commands",
            Self::Clipboard => "Clipboard",
            Self::Integrations => "Integrations",
            Self::Dictionary => "Dictionary",
            Self::Snippets => "Snippets",
            Self::Style => "Style",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayState {
    #[default]
    Idle,
    Listening,
    Handsfree,
    Processing,
    ConfirmationRequired,
    Success,
    Error,
}

#[derive(Clone, Debug)]
pub struct RuntimeUiState {
    pub model: String,
    pub transcription_phase: String,
    pub audio_phase: String,
    pub audio_device: String,
    pub audio_level: f32,
    pub overlay_detail: String,
    pub last_transcription: String,
    pub shortcut_status: String,
}

impl Default for RuntimeUiState {
    fn default() -> Self {
        Self {
            model: "Checking…".to_string(),
            transcription_phase: "Starting".to_string(),
            audio_phase: "Idle".to_string(),
            audio_device: "Default microphone".to_string(),
            audio_level: 0.0,
            overlay_detail: String::new(),
            last_transcription: "No transcription yet".to_string(),
            shortcut_status: "Registering shortcuts".to_string(),
        }
    }
}

impl OverlayState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Listening => "Listening",
            Self::Handsfree => "Hands-free",
            Self::Processing => "Processing",
            Self::ConfirmationRequired => "Confirm",
            Self::Success => "Success",
            Self::Error => "Error",
        }
    }
}
