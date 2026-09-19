//! Typed execution primitives for CLI commands that can operate directly on
//! the reusable Rust core without constructing GPUI dashboard state.
//!
//! Process lifecycle, autostart, package replacement and runtime reload belong
//! to the platform/bootstrap layer. These methods cover setup, config, models,
//! microphones and persisted hotkeys.

use crate::cli::{
    CommandResult, ConfigAction, DictionaryAction, HotkeyAction, MicAction, ModelAction,
    SetupOptions, ToggleAction,
};
use crate::system_capabilities::SystemCapabilities;
use origin_speak_lib::{
    AppConfig, AppState, State, add_dictionary_word, delete_dictionary_word, delete_local_model,
    download_local_model, get_audio_devices, get_config, get_dictionary_words,
    get_transcription_settings, get_trigger_hotkey, list_local_models, set_audio_device,
    set_config, set_transcription_model, set_trigger_hotkey, update_dictionary_word,
};
use std::sync::Arc;

pub struct CoreCliExecutor {
    state: Arc<AppState>,
}

impl Default for CoreCliExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreCliExecutor {
    pub fn new() -> Self {
        Self {
            state: Arc::new(AppState::default()),
        }
    }

    pub fn state(&self) -> Arc<AppState> {
        self.state.clone()
    }

    /// Idempotent first-run setup. Existing downloaded models and matching
    /// settings are retained. When no model is supplied, verified CPU/RAM data
    /// chooses a conservative latency-oriented model.
    pub async fn setup(&self, options: &SetupOptions) -> Result<CommandResult, String> {
        let capabilities = SystemCapabilities::detect();
        let recommendation = capabilities.recommend_model();
        let model = options.model.as_deref().unwrap_or(recommendation.model);

        let models = list_local_models(State::new(self.state.as_ref())).await?;
        let selected = get_transcription_settings(State::new(self.state.as_ref()))
            .await?
            .model;
        let model_info = models
            .iter()
            .find(|entry| entry.id == model)
            .ok_or_else(|| format!("Unknown local transcription model: {model}"))?;
        let mut downloaded = false;
        if !model_info.downloaded {
            download_local_model(State::new(self.state.as_ref()), model.to_string()).await?;
            downloaded = true;
        }
        if selected != model {
            set_transcription_model(State::new(self.state.as_ref()), model.to_string()).await?;
        }

        if let Some(microphone) = options.microphone.as_deref() {
            let config = get_config(State::new(self.state.as_ref())).await?;
            if is_system_default_microphone(microphone) {
                if config.selected_audio_device.is_some() {
                    let mut config = config;
                    config.selected_audio_device = None;
                    set_config(State::new(self.state.as_ref()), config).await?;
                }
            } else if config.selected_audio_device.as_deref() != Some(microphone) {
                set_audio_device(State::new(self.state.as_ref()), microphone.to_string()).await?;
            }
        }

        if let Some(hotkey) = options.hotkey.as_deref() {
            let current = get_trigger_hotkey(State::new(self.state.as_ref())).await?;
            if current != hotkey {
                set_trigger_hotkey(State::new(self.state.as_ref()), hotkey.to_string()).await?;
            }
        }

        let mut config = get_config(State::new(self.state.as_ref())).await?;
        if options
            .autostart
            .is_some_and(|enabled| config.auto_start != enabled)
        {
            config.auto_start = options.autostart.unwrap_or(config.auto_start);
            set_config(State::new(self.state.as_ref()), config).await?;
        }

        let memory = capabilities
            .memory_gib()
            .map(|value| format!("{value} GiB"))
            .unwrap_or_else(|| "unknown".to_string());
        let mut result = CommandResult::success("setup_complete", "Origin Speak setup is ready")
            .field("model", model)
            .field("model_downloaded_now", downloaded.to_string())
            .field("cpu_arch", capabilities.arch)
            .field("logical_cpus", capabilities.logical_cpus.to_string())
            .field("memory", memory)
            .field("accelerator", capabilities.accelerator.to_string())
            .field("recommendation", recommendation.rationale);
        if let Some(microphone) = options.microphone.as_deref() {
            result = result.field(
                "microphone",
                if is_system_default_microphone(microphone) {
                    "system default"
                } else {
                    microphone
                },
            );
        }
        if let Some(hotkey) = options.hotkey.as_deref() {
            result = result.field("hotkey", hotkey);
        }
        if let Some(autostart) = options.autostart {
            result = result.field("autostart", autostart.to_string());
        }
        Ok(result)
    }

    pub async fn model(&self, action: &ModelAction) -> Result<CommandResult, String> {
        match action {
            ModelAction::List => {
                let models = list_local_models(State::new(self.state.as_ref())).await?;
                let selected = get_transcription_settings(State::new(self.state.as_ref()))
                    .await?
                    .model;
                let summary = models
                    .iter()
                    .map(|model| {
                        format!(
                            "{}{}{}",
                            model.id,
                            if model.id == selected { "*" } else { "" },
                            if model.downloaded {
                                " (downloaded)"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                Ok(
                    CommandResult::success("models", "Local transcription models")
                        .field("models", summary),
                )
            }
            ModelAction::Status => {
                let selected = get_transcription_settings(State::new(self.state.as_ref()))
                    .await?
                    .model;
                let config = get_config(State::new(self.state.as_ref())).await?;
                let models = list_local_models(State::new(self.state.as_ref())).await?;
                let downloaded = models
                    .iter()
                    .find(|model| model.id == selected)
                    .is_some_and(|model| model.downloaded);
                Ok(
                    CommandResult::success("model_status", "Transcription model status")
                        .field("selected", selected)
                        .field("downloaded", downloaded.to_string())
                        .field(
                            "gpu_preference",
                            if config.use_gpu {
                                "enabled"
                            } else {
                                "disabled"
                            },
                        ),
                )
            }
            ModelAction::Select(model) => {
                let current = get_transcription_settings(State::new(self.state.as_ref()))
                    .await?
                    .model;
                if current != *model {
                    set_transcription_model(State::new(self.state.as_ref()), model.clone()).await?;
                }
                Ok(CommandResult::success(
                    "model_selected",
                    format!("Selected model {model}"),
                ))
            }
            ModelAction::Download(model) => {
                let existing = list_local_models(State::new(self.state.as_ref())).await?;
                let downloaded = existing
                    .iter()
                    .find(|entry| entry.id == *model)
                    .is_some_and(|entry| entry.downloaded);
                if !downloaded {
                    download_local_model(State::new(self.state.as_ref()), model.clone()).await?;
                }
                Ok(CommandResult::success(
                    "model_downloaded",
                    format!("Model {model} is available"),
                )
                .field("downloaded_now", (!downloaded).to_string()))
            }
            ModelAction::Remove(model) => {
                let selected = get_transcription_settings(State::new(self.state.as_ref()))
                    .await?
                    .model;
                ensure_model_removable(&selected, model)?;
                let removed =
                    delete_local_model(State::new(self.state.as_ref()), model.clone()).await?;
                Ok(CommandResult::success(
                    "model_removed",
                    format!("Model {model} removal complete"),
                )
                .field("removed", removed.to_string()))
            }
        }
    }

    pub async fn dictionary(&self, action: &DictionaryAction) -> Result<CommandResult, String> {
        match action {
            DictionaryAction::List => {
                let words = get_dictionary_words().await?;
                let summary = if words.is_empty() {
                    "No recognition dictionary entries.".to_string()
                } else {
                    words
                        .iter()
                        .map(|entry| {
                            format!(
                                "{}\t{}{}",
                                entry.id,
                                entry.word,
                                entry
                                    .phonetic
                                    .as_deref()
                                    .map(|value| format!("\t{value}"))
                                    .unwrap_or_default()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                Ok(CommandResult::success("dictionary", summary)
                    .field("count", words.len().to_string()))
            }
            DictionaryAction::Add {
                word,
                pronunciation,
            } => {
                let word = validated_dictionary_text("word", word)?;
                let pronunciation = normalized_optional_text(pronunciation.as_deref());
                let added = add_dictionary_word(word.clone(), false).await?;
                if let Some(pronunciation) = pronunciation.clone() {
                    if let Err(error) = update_dictionary_word(
                        added.id.clone(),
                        word.clone(),
                        Some(pronunciation.clone()),
                    )
                    .await
                    {
                        let _ = delete_dictionary_word(added.id.clone()).await;
                        return Err(error);
                    }
                }
                Ok(CommandResult::success(
                    "dictionary_added",
                    format!("Added '{word}' to the recognition dictionary"),
                )
                .field("id", added.id)
                .field("word", word)
                .field(
                    "pronunciation",
                    pronunciation.unwrap_or_else(|| "none".to_string()),
                ))
            }
            DictionaryAction::Update {
                id,
                word,
                pronunciation,
            } => {
                let id = validated_dictionary_text("id", id)?;
                let word = validated_dictionary_text("word", word)?;
                let pronunciation = normalized_optional_text(pronunciation.as_deref());
                update_dictionary_word(id.clone(), word.clone(), pronunciation.clone()).await?;
                Ok(CommandResult::success(
                    "dictionary_updated",
                    format!("Updated recognition dictionary entry '{word}'"),
                )
                .field("id", id)
                .field("word", word)
                .field(
                    "pronunciation",
                    pronunciation.unwrap_or_else(|| "none".to_string()),
                ))
            }
            DictionaryAction::Remove(id) => {
                let id = validated_dictionary_text("id", id)?;
                delete_dictionary_word(id.clone()).await?;
                Ok(CommandResult::success(
                    "dictionary_removed",
                    "Removed recognition dictionary entry",
                )
                .field("id", id))
            }
        }
    }

    pub async fn mic(&self, action: &MicAction) -> Result<CommandResult, String> {
        match action {
            MicAction::List => {
                let devices = get_audio_devices().await?;
                let summary = devices
                    .iter()
                    .map(|device| {
                        format!(
                            "{}{}",
                            device.name,
                            if device.is_default { " (default)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                Ok(
                    CommandResult::success("microphones", "Available microphones")
                        .field("devices", summary),
                )
            }
            MicAction::Status => {
                let config = get_config(State::new(self.state.as_ref())).await?;
                Ok(
                    CommandResult::success("microphone_status", "Microphone selection").field(
                        "selected",
                        config
                            .selected_audio_device
                            .unwrap_or_else(|| "system default".to_string()),
                    ),
                )
            }
            MicAction::Select(name) => {
                let config = get_config(State::new(self.state.as_ref())).await?;
                if is_system_default_microphone(name) {
                    if config.selected_audio_device.is_some() {
                        let mut config = config;
                        config.selected_audio_device = None;
                        set_config(State::new(self.state.as_ref()), config).await?;
                    }
                    return Ok(CommandResult::success(
                        "microphone_selected",
                        "Selected system default microphone",
                    )
                    .field("selected", "system default"));
                }
                if config.selected_audio_device.as_deref() != Some(name) {
                    set_audio_device(State::new(self.state.as_ref()), name.clone()).await?;
                }
                Ok(CommandResult::success(
                    "microphone_selected",
                    format!("Selected microphone {name}"),
                ))
            }
        }
    }

    pub async fn hotkey(&self, action: &HotkeyAction) -> Result<CommandResult, String> {
        match action {
            HotkeyAction::Show => {
                Ok(
                    CommandResult::success("hotkey", "Dictation shortcut configuration").field(
                        "trigger",
                        get_trigger_hotkey(State::new(self.state.as_ref())).await?,
                    ),
                )
            }
            HotkeyAction::Set(chord) => {
                let normalized =
                    set_trigger_hotkey(State::new(self.state.as_ref()), chord.clone()).await?;
                Ok(
                    CommandResult::success("hotkey_saved", "Dictation hotkey saved")
                        .field("hotkey", normalized),
                )
            }
        }
    }

    pub async fn autostart(&self, action: ToggleAction) -> Result<CommandResult, String> {
        let mut config = get_config(State::new(self.state.as_ref())).await?;
        match action {
            ToggleAction::Status => Ok(CommandResult::success(
                "autostart_status",
                "Autostart preference",
            )
            .field("enabled", config.auto_start.to_string())),
            ToggleAction::Enable | ToggleAction::Disable => {
                let enabled = matches!(action, ToggleAction::Enable);
                if config.auto_start != enabled {
                    config.auto_start = enabled;
                    set_config(State::new(self.state.as_ref()), config).await?;
                }
                Ok(CommandResult::success(
                    "autostart_saved",
                    "Autostart preference saved; the resident runtime applies native registration",
                )
                .field("enabled", enabled.to_string()))
            }
        }
    }

    pub async fn config(&self, action: &ConfigAction) -> Result<CommandResult, String> {
        match action {
            ConfigAction::List => {
                let config = get_config(State::new(self.state.as_ref())).await?;
                let value = serde_json::to_string(&config)
                    .map_err(|error| format!("serialize config: {error}"))?;
                Ok(CommandResult::success("config", "Persisted configuration")
                    .field("value", value))
            }
            ConfigAction::Get(path) => {
                let config = get_config(State::new(self.state.as_ref())).await?;
                let value = serde_json::to_value(config)
                    .map_err(|error| format!("serialize config: {error}"))?;
                let selected =
                    json_path(&value, path).ok_or_else(|| format!("Unknown config key: {path}"))?;
                Ok(CommandResult::success(
                    "config_value",
                    format!("{path} = {}", json_scalar(selected)?),
                )
                .field("key", path)
                .field("value", json_scalar(selected)?))
            }
            ConfigAction::Set { key, value } => {
                let config = get_config(State::new(self.state.as_ref())).await?;
                let mut json = serde_json::to_value(config)
                    .map_err(|error| format!("serialize config: {error}"))?;
                let parsed = parse_cli_value(value);
                *json_path_mut(&mut json, key)
                    .ok_or_else(|| format!("Unknown config key: {key}"))? = parsed;
                let config: AppConfig = serde_json::from_value(json)
                    .map_err(|error| format!("Invalid value for {key}: {error}"))?;
                set_config(State::new(self.state.as_ref()), config).await?;
                Ok(CommandResult::success(
                    "config_saved",
                    format!("Saved {key}"),
                ))
            }
            ConfigAction::Reset(key) => {
                let config = get_config(State::new(self.state.as_ref())).await?;
                let mut current = serde_json::to_value(config)
                    .map_err(|error| format!("serialize config: {error}"))?;
                let defaults = serde_json::to_value(AppConfig::default())
                    .map_err(|error| format!("serialize default config: {error}"))?;
                let default = json_path(&defaults, key)
                    .cloned()
                    .ok_or_else(|| format!("Unknown config key: {key}"))?;
                *json_path_mut(&mut current, key)
                    .ok_or_else(|| format!("Unknown config key: {key}"))? = default;
                let config: AppConfig = serde_json::from_value(current)
                    .map_err(|error| format!("restore default {key}: {error}"))?;
                set_config(State::new(self.state.as_ref()), config).await?;
                Ok(CommandResult::success(
                    "config_reset",
                    format!("Reset {key} to its default"),
                ))
            }
        }
    }
}

fn parse_cli_value(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
}

fn is_system_default_microphone(value: &str) -> bool {
    value.eq_ignore_ascii_case("default") || value.eq_ignore_ascii_case("system default")
}

fn ensure_model_removable(selected: &str, target: &str) -> Result<(), String> {
    if selected == target {
        return Err(format!(
            "Cannot remove the currently selected model '{target}'. Select another model first."
        ));
    }
    Ok(())
}

fn validated_dictionary_text(label: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("Dictionary {label} cannot be empty"));
    }
    Ok(value.to_string())
}

fn normalized_optional_text(value: Option<&str>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn json_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    path.split('.')
        .try_fold(value, |current, segment| current.get(segment))
}

fn json_path_mut<'a>(
    value: &'a mut serde_json::Value,
    path: &str,
) -> Option<&'a mut serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get_mut(segment)?;
    }
    Some(current)
}

fn json_scalar(value: &serde_json::Value) -> Result<String, String> {
    match value {
        serde_json::Value::String(value) => Ok(value.clone()),
        other => {
            serde_json::to_string(other).map_err(|error| format!("serialize config value: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_config_path_mutation_is_precise() {
        let mut value = serde_json::json!({"ui": {"show_overlay": true}, "auto_copy": false});
        *json_path_mut(&mut value, "ui.show_overlay").unwrap() = serde_json::Value::Bool(false);
        assert_eq!(
            json_path(&value, "ui.show_overlay"),
            Some(&serde_json::Value::Bool(false))
        );
        assert_eq!(
            json_path(&value, "auto_copy"),
            Some(&serde_json::Value::Bool(false))
        );
    }

    #[test]
    fn cli_values_accept_json_or_plain_strings() {
        assert_eq!(parse_cli_value("true"), serde_json::Value::Bool(true));
        assert_eq!(
            parse_cli_value("hello world"),
            serde_json::Value::String("hello world".into())
        );
    }

    #[test]
    fn setup_default_microphone_sentinel_is_explicit() {
        assert!(is_system_default_microphone("default"));
        assert!(is_system_default_microphone("SYSTEM DEFAULT"));
        assert!(!is_system_default_microphone("Studio Mic"));
    }

    #[test]
    fn selected_model_cannot_be_removed() {
        let error = ensure_model_removable("base.en", "base.en").unwrap_err();
        assert!(error.contains("Select another model first"));
        assert!(ensure_model_removable("base.en", "tiny.en").is_ok());
    }

    #[test]
    fn dictionary_values_are_trimmed_and_empty_pronunciation_is_none() {
        assert_eq!(
            validated_dictionary_text("word", "  Axius  ").unwrap(),
            "Axius"
        );
        assert!(validated_dictionary_text("word", "  ").is_err());
        assert_eq!(
            normalized_optional_text(Some("  ACK-see-us  ")),
            Some("ACK-see-us".into())
        );
        assert_eq!(normalized_optional_text(Some("   ")), None);
    }
}
