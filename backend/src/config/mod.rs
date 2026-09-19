//! Persisted configuration for the dictation-only backend.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AppConfig {
    pub trigger_hotkey: String,
    pub selected_audio_device: Option<String>,
    pub use_gpu: bool,
    pub auto_start: bool,
    pub language_preferences: LanguagePreferences,
}

impl AppConfig {
    fn storage_path() -> Result<PathBuf, String> {
        Ok(crate::app_data_root()?.join("config.json"))
    }

    pub fn load_from_disk() -> Option<Self> {
        let active_path = Self::storage_path().ok()?;
        let active_backup = active_path.with_extension("json.bak");
        for candidate in [&active_path, &active_backup] {
            if !crate::paths::regular_file_exists_no_follow(candidate) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(candidate) else {
                continue;
            };
            match serde_json::from_str::<Self>(&content) {
                Ok(config) => return Some(config),
                Err(error) => log::warn!(
                    "Ignoring invalid Origin Speak config at {}: {}",
                    candidate.display(),
                    error
                ),
            }
        }

        if crate::paths::path_exists_no_follow(&active_path)
            || crate::paths::path_exists_no_follow(&active_backup)
        {
            return None;
        }

        let paths = crate::paths::app_data_file_candidates("config.json").ok()?;
        for path in paths.into_iter().skip(1) {
            for candidate in [&path, &path.with_extension("json.bak")] {
                if !crate::paths::regular_file_exists_no_follow(candidate) {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(candidate) else {
                    continue;
                };
                match serde_json::from_str::<Self>(&content) {
                    Ok(config) => {
                        if let Err(error) = config.save_to_disk() {
                            log::warn!(
                                "Loaded legacy ListenOS config from {}, but could not migrate it to Origin Speak: {}",
                                candidate.display(),
                                error
                            );
                        }
                        return Some(config);
                    }
                    Err(error) => log::warn!(
                        "Ignoring invalid Origin Speak config at {}: {}",
                        candidate.display(),
                        error
                    ),
                }
            }
        }
        None
    }

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
                .map_err(|error| format!("Failed to sync temporary config: {error}"))?;
        }

        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|error| format!("Failed to replace application config: {error}"))?;
        }
        std::fs::rename(&temp_path, &path)
            .map_err(|error| format!("Failed to install application config: {error}"))
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        let trigger_hotkey = if cfg!(target_os = "macos") {
            "Ctrl+Space".to_string()
        } else {
            "Meta+Ctrl+Space".to_string()
        };
        Self {
            trigger_hotkey,
            selected_audio_device: None,
            use_gpu: true,
            auto_start: true,
            language_preferences: LanguagePreferences::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LanguagePreferences {
    /// Whisper source-language hint. `auto` lets Whisper infer the language.
    pub source_language: String,
}

impl LanguagePreferences {
    pub fn transcription_language_hint(&self) -> Option<&str> {
        let value = self.source_language.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("auto") {
            None
        } else {
            Some(value)
        }
    }
}

impl Default for LanguagePreferences {
    fn default() -> Self {
        Self {
            source_language: "en".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hold_to_talk_avoids_os_reserved_chords() {
        let expected = if cfg!(target_os = "macos") {
            "Ctrl+Space"
        } else {
            "Meta+Ctrl+Space"
        };
        assert_eq!(AppConfig::default().trigger_hotkey, expected);
    }

    #[test]
    fn old_rich_config_preserves_only_current_voice_to_text_fields() {
        let legacy = serde_json::json!({
            "trigger_hotkey": "Ctrl+Shift+Space",
            "assistant_hotkey": "Ctrl+Alt+Space",
            "listening_mode": "Toggle",
            "auto_copy": false,
            "selected_audio_device": "Studio Mic",
            "use_gpu": false,
            "ui": {
                "theme": "light",
                "onboarding_completed": true,
                "overlay_position": "TopRight"
            },
            "sound_feedback": false,
            "auto_start": false,
            "dictation_style": {"personal": "Casual", "work": "Formal", "email": "Formal", "other": "Formal"},
            "language_preferences": {
                "source_language": "hi",
                "target_language": "en"
            },
            "vibe_coding": {"enabled": true, "trigger_phrase": "vibe"}
        });

        let config: AppConfig = serde_json::from_value(legacy).expect("migrate rich config");
        assert_eq!(config.trigger_hotkey, "Ctrl+Shift+Space");
        assert_eq!(config.selected_audio_device.as_deref(), Some("Studio Mic"));
        assert!(!config.use_gpu);
        assert!(!config.auto_start);
        assert_eq!(config.language_preferences.source_language, "hi");

        let serialized = serde_json::to_value(config).expect("serialize compact config");
        let object = serialized.as_object().expect("config object");
        for removed in [
            "assistant_hotkey",
            "listening_mode",
            "auto_copy",
            "ui",
            "sound_feedback",
            "dictation_style",
            "vibe_coding",
        ] {
            assert!(
                !object.contains_key(removed),
                "legacy field leaked: {removed}"
            );
        }
        assert_eq!(
            object["language_preferences"].as_object().unwrap().len(),
            1,
            "translation/output-language settings must not survive dictation-only migration"
        );
    }

    #[test]
    fn missing_new_fields_default_when_loading_legacy_config() {
        let config: AppConfig = serde_json::from_value(serde_json::json!({
            "trigger_hotkey": "Ctrl+Space"
        }))
        .expect("load sparse legacy config");
        assert_eq!(config.trigger_hotkey, "Ctrl+Space");
        assert_eq!(config.selected_audio_device, None);
        assert!(config.use_gpu);
        assert!(config.auto_start);
        assert_eq!(config.language_preferences, LanguagePreferences::default());
    }
}
