//! Application configuration module

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Hotkey to trigger listening (e.g., "Ctrl+Space")
    pub trigger_hotkey: String,

    /// Hotkey to toggle assistant handsfree listening (e.g., "Ctrl+Alt+Space")
    #[serde(default = "default_assistant_hotkey")]
    pub assistant_hotkey: String,

    /// Mode: "push_to_talk" or "toggle"
    pub listening_mode: ListeningMode,

    /// Auto-copy transcription to clipboard
    pub auto_copy: bool,

    /// Preferred microphone. `None` follows the current OS default device.
    #[serde(default)]
    pub selected_audio_device: Option<String>,

    /// Prefer hardware acceleration for local transcription when available.
    /// Older configs did not have this field, so migration keeps the product
    /// GPU-first instead of silently pinning upgraded installs to CPU.
    #[serde(default = "default_use_gpu")]
    pub use_gpu: bool,

    /// UI preferences
    pub ui: UIConfig,

    /// Sound feedback enabled
    pub sound_feedback: bool,

    /// Auto-start on system boot
    pub auto_start: bool,

    /// Dictation style settings per context
    pub dictation_style: DictationStyleConfig,

    /// Speech input and output language preferences for multilingual mode
    #[serde(default)]
    pub language_preferences: LanguagePreferences,

    /// Vibe coding prompt formatting settings
    #[serde(default)]
    pub vibe_coding: VibeCodingConfig,
}

impl AppConfig {
    fn storage_path() -> Result<PathBuf, String> {
        let data_dir =
            dirs_next::data_dir().ok_or_else(|| "Could not find data directory".to_string())?;
        Ok(data_dir.join("ListenOS").join("config.json"))
    }

    /// Load the authoritative desktop configuration used by native frontends.
    ///
    /// Older ListenOS builds persisted only a few sub-settings. Callers may
    /// still merge those legacy files after this returns so existing installs
    /// retain their language and vibe preferences during the GPUI migration.
    pub fn load_from_disk() -> Option<Self> {
        let path = Self::storage_path().ok()?;
        for candidate in [&path, &path.with_extension("json.bak")] {
            let Ok(content) = std::fs::read_to_string(candidate) else {
                continue;
            };
            match serde_json::from_str::<Self>(&content) {
                Ok(config) => return Some(config),
                Err(error) => {
                    log::warn!(
                        "Ignoring invalid ListenOS config at {}: {}",
                        candidate.display(),
                        error
                    );
                }
            }
        }
        None
    }

    /// Persist the complete application configuration in the OS user-data
    /// directory. The GPUI frontend calls the typed Rust config commands, so
    /// settings no longer need browser localStorage to survive a restart.
    pub fn save_to_disk(&self) -> Result<(), String> {
        let path = Self::storage_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create config directory: {error}"))?;
        }

        let payload = serde_json::to_string_pretty(self)
            .map_err(|error| format!("Failed to serialize application config: {error}"))?;
        let temp_path = path.with_extension("json.tmp");
        let backup_path = path.with_extension("json.bak");

        // Keep one last-known-good copy so a power loss during replacement does
        // not silently reset all native settings on the next launch.
        if let Ok(existing) = std::fs::read_to_string(&path) {
            if serde_json::from_str::<Self>(&existing).is_ok() {
                std::fs::write(&backup_path, existing)
                    .map_err(|error| format!("Failed to back up application config: {error}"))?;
            }
        }

        {
            let mut file = std::fs::File::create(&temp_path)
                .map_err(|error| format!("Failed to create temporary config: {error}"))?;
            file.write_all(payload.as_bytes())
                .map_err(|error| format!("Failed to write temporary config: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("Failed to flush temporary config: {error}"))?;
        }

        match std::fs::rename(&temp_path, &path) {
            Ok(()) => Ok(()),
            Err(rename_error) if path.exists() => {
                // Windows does not replace an existing destination with
                // `std::fs::rename`. The validated backup above provides a
                // recovery source across the short remove/rename window.
                std::fs::remove_file(&path)
                    .map_err(|error| format!("Failed to replace application config: {error}"))?;
                std::fs::rename(&temp_path, &path).map_err(|error| {
                    format!("Failed to replace application config after {rename_error}: {error}")
                })
            }
            Err(error) => Err(format!("Failed to replace application config: {error}")),
        }
    }
}

/// Multilingual language preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguagePreferences {
    /// Input speech language ("auto", "en", "hi", etc.)
    pub source_language: String,
    /// Output text language ("en", "hi", etc.)
    pub target_language: String,
}

impl LanguagePreferences {
    /// Language hint to pass to STT (None means auto-detect).
    pub fn transcription_language_hint(&self) -> Option<&str> {
        let source = self.source_language.trim().to_lowercase();
        if source.is_empty() || source == "auto" {
            None
        } else {
            Some(self.source_language.as_str())
        }
    }

    fn storage_path() -> Result<PathBuf, String> {
        let data_dir =
            dirs_next::data_dir().ok_or_else(|| "Could not find data directory".to_string())?;
        Ok(data_dir.join("ListenOS").join("language_preferences.json"))
    }

    pub fn load_from_disk() -> Option<Self> {
        let path = Self::storage_path().ok()?;
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<Self>(&content).ok()
    }

    pub fn save_to_disk(&self) -> Result<(), String> {
        let path = Self::storage_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create preferences directory: {}", e))?;
        }

        let payload = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize language preferences: {}", e))?;
        std::fs::write(&path, payload)
            .map_err(|e| format!("Failed to write language preferences: {}", e))?;
        Ok(())
    }
}

impl Default for LanguagePreferences {
    fn default() -> Self {
        Self {
            source_language: "en".to_string(),
            target_language: "en".to_string(),
        }
    }
}

/// Controls for organizing spoken coding prompts before delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VibeCodingConfig {
    /// Master toggle.
    pub enabled: bool,
    /// Activation behavior.
    pub activation_mode: VibeActivationMode,
    /// Spoken prefix used when trigger-phrase activation is selected.
    pub trigger_phrase: String,
    /// Presentation applied without changing the requested work.
    pub formatting: VibeFormattingStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VibeActivationMode {
    /// Organize prompts when the active app or spoken request is clearly coding-related.
    #[serde(alias = "SmartAuto", alias = "Always")]
    Automatic,
    /// Organize only when the configured trigger phrase is spoken.
    #[serde(alias = "ManualOnly")]
    TriggerPhrase,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VibeFormattingStyle {
    /// Keep the user's wording as natural prose.
    #[default]
    Natural,
    /// Lay out the user's own clauses as a scan-friendly list.
    Structured,
}

impl VibeCodingConfig {
    fn storage_path() -> Result<PathBuf, String> {
        let data_dir =
            dirs_next::data_dir().ok_or_else(|| "Could not find data directory".to_string())?;
        Ok(data_dir.join("ListenOS").join("vibe_coding.json"))
    }

    pub fn load_from_disk() -> Option<Self> {
        let path = Self::storage_path().ok()?;
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<Self>(&content).ok()
    }

    pub fn save_to_disk(&self) -> Result<(), String> {
        let path = Self::storage_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create vibe config directory: {}", e))?;
        }

        let payload = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize vibe coding config: {}", e))?;
        std::fs::write(&path, payload)
            .map_err(|e| format!("Failed to write vibe coding config: {}", e))?;
        Ok(())
    }
}

impl Default for VibeCodingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            activation_mode: VibeActivationMode::Automatic,
            trigger_phrase: "vibe".to_string(),
            formatting: VibeFormattingStyle::Natural,
        }
    }
}

/// Dictation style configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictationStyleConfig {
    /// Style for personal messages (messengers)
    pub personal: DictationStyle,
    /// Style for work messages (Slack, Teams)
    pub work: DictationStyle,
    /// Style for email
    pub email: DictationStyle,
    /// Style for other contexts
    pub other: DictationStyle,
}

impl Default for DictationStyleConfig {
    fn default() -> Self {
        Self {
            personal: DictationStyle::Casual,
            work: DictationStyle::Formal,
            email: DictationStyle::Formal,
            other: DictationStyle::Formal,
        }
    }
}

/// Dictation style affects capitalization and punctuation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DictationStyle {
    /// Caps + Full punctuation
    Formal,
    /// Caps + Less punctuation
    Casual,
    /// No caps + Less punctuation  
    VeryCasual,
}

impl Default for AppConfig {
    fn default() -> Self {
        // Windows/Linux default to Win+Ctrl+Space: Win+Space alone flips
        // keyboard layouts on Windows, so the extra Ctrl keeps the chord
        // clear of the OS. macOS keeps Ctrl+Space because Ctrl+Cmd+Space
        // is the system character viewer. Users can customize both bindings
        // in settings; existing installs keep their saved shortcuts.
        let default_hotkey = if cfg!(target_os = "macos") {
            "Ctrl+Space".to_string()
        } else {
            "Meta+Ctrl+Space".to_string()
        };

        Self {
            trigger_hotkey: default_hotkey,
            assistant_hotkey: default_assistant_hotkey(),
            listening_mode: ListeningMode::PushToTalk,
            auto_copy: true,
            selected_audio_device: None,
            use_gpu: default_use_gpu(),
            ui: UIConfig::default(),
            sound_feedback: true,
            auto_start: true,
            dictation_style: DictationStyleConfig::default(),
            language_preferences: LanguagePreferences::default(),
            vibe_coding: VibeCodingConfig::default(),
        }
    }
}

/// Listening mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListeningMode {
    /// Hold hotkey to listen, release to process
    PushToTalk,
    /// Press once to start, press again to stop
    Toggle,
    /// Continuous listening with wake word
    VoiceActivated,
}

fn default_assistant_hotkey() -> String {
    "Ctrl+Alt+Space".to_string()
}

fn default_use_gpu() -> bool {
    true
}

/// UI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UIConfig {
    /// Theme: "dark" or "light"
    pub theme: String,

    /// Accent color (hex)
    pub accent_color: String,

    /// Window opacity (0.0 - 1.0)
    pub opacity: f32,

    /// Show transcription in overlay
    pub show_transcription: bool,

    /// Overlay position
    pub overlay_position: OverlayPosition,

    /// Window size
    pub window_width: u32,
    pub window_height: u32,

    /// First-run native setup has completed.
    #[serde(default)]
    pub onboarding_completed: bool,
}

impl Default for UIConfig {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            accent_color: "#06b6d4".to_string(), // Cyan
            opacity: 0.9,
            show_transcription: true,
            overlay_position: OverlayPosition::BottomCenter,
            window_width: 400,
            window_height: 600,
            onboarding_completed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_native_config_defaults_new_migration_fields() {
        let mut value =
            serde_json::to_value(AppConfig::default()).expect("serialize default config");
        let object = value.as_object_mut().expect("config object");
        object.remove("selected_audio_device");
        object.remove("use_gpu");
        object
            .get_mut("ui")
            .and_then(serde_json::Value::as_object_mut)
            .expect("ui object")
            .remove("onboarding_completed");

        let config: AppConfig = serde_json::from_value(value).expect("load pre-native config");
        assert_eq!(config.selected_audio_device, None);
        assert!(config.use_gpu, "upgraded installs must remain GPU-first");
        assert!(!config.ui.onboarding_completed);
    }

    #[test]
    fn default_hold_to_talk_avoids_os_reserved_chords() {
        // Win+Space flips keyboard layouts on Windows and Ctrl+Cmd+Space is
        // the macOS character viewer, so each platform gets a clear default.
        let expected = if cfg!(target_os = "macos") {
            "Ctrl+Space"
        } else {
            "Meta+Ctrl+Space"
        };
        assert_eq!(AppConfig::default().trigger_hotkey, expected);
    }

    #[test]
    fn legacy_vibe_config_migrates_to_simplified_contract() {
        let legacy = serde_json::json!({
            "enabled": true,
            "activation_mode": "ManualOnly",
            "trigger_phrase": "ship it",
            "target_tool": "Cursor",
            "detail_level": "Detailed",
            "include_constraints": true,
            "include_acceptance_criteria": true,
            "include_test_notes": true,
            "concise_output": false
        });

        let config: VibeCodingConfig =
            serde_json::from_value(legacy).expect("load legacy vibe config");
        assert!(config.enabled);
        assert_eq!(config.activation_mode, VibeActivationMode::TriggerPhrase);
        assert_eq!(config.trigger_phrase, "ship it");
        assert_eq!(config.formatting, VibeFormattingStyle::Natural);

        let serialized = serde_json::to_value(config).expect("serialize migrated vibe config");
        let object = serialized.as_object().expect("vibe config object");
        assert!(object.contains_key("formatting"));
        for removed in [
            "target_tool",
            "detail_level",
            "include_constraints",
            "include_acceptance_criteria",
            "include_test_notes",
            "concise_output",
        ] {
            assert!(
                !object.contains_key(removed),
                "legacy field {removed} leaked"
            );
        }
    }

    #[test]
    fn legacy_automatic_modes_deserialize_as_coding_context_activation() {
        for legacy_mode in ["SmartAuto", "Always"] {
            let config: VibeCodingConfig = serde_json::from_value(serde_json::json!({
                "enabled": true,
                "activation_mode": legacy_mode,
                "trigger_phrase": "vibe"
            }))
            .expect("load legacy automatic mode");
            assert_eq!(config.activation_mode, VibeActivationMode::Automatic);
        }
    }
}

/// Overlay position on screen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlayPosition {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
    Center,
}
