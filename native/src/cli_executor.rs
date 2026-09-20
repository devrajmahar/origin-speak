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
    AppConfig, AppState, LocalModelInfo, State, add_dictionary_word, delete_dictionary_word,
    delete_local_model, download_local_model, get_audio_devices, get_config, get_dictionary_words,
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
            ModelAction::Help => Ok(CommandResult::success(
                "model_help",
                "Origin Speak model management help",
            )
            .human(model_help())),
            ModelAction::List => {
                let models = list_local_models(State::new(self.state.as_ref())).await?;
                let human = render_model_catalog(&models, terminal_width());
                Ok(
                    CommandResult::success("models", "Available transcription models")
                        .field("models", render_models(&models))
                        .field(
                            "installed_count",
                            models
                                .iter()
                                .filter(|model| model.downloaded)
                                .count()
                                .to_string(),
                        )
                        .human(human),
                )
            }
            ModelAction::Installed => {
                let models = list_local_models(State::new(self.state.as_ref())).await?;
                let installed = models
                    .iter()
                    .filter(|model| model.downloaded)
                    .cloned()
                    .collect::<Vec<_>>();
                let summary = if installed.is_empty() {
                    "No transcription models are installed.".to_string()
                } else {
                    render_models(&installed)
                };
                let human = render_installed_models(&installed, terminal_width());
                Ok(
                    CommandResult::success("installed_models", "Installed transcription models")
                        .field("models", summary)
                        .field("installed_count", installed.len().to_string())
                        .human(human),
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
                let models = list_local_models(State::new(self.state.as_ref())).await?;
                let info = models
                    .iter()
                    .find(|entry| entry.id == *model)
                    .ok_or_else(|| unknown_model_error(model, &models))?;
                let installed_now = !info.downloaded;
                if installed_now {
                    download_local_model(State::new(self.state.as_ref()), model.clone())
                        .await
                        .map_err(|error| model_install_error(model, error))?;
                }
                if current != *model {
                    set_transcription_model(State::new(self.state.as_ref()), model.clone()).await?;
                }
                Ok(CommandResult::success(
                    "model_selected",
                    format!("Model {model} is now the default"),
                )
                .field("selected", model)
                .field("installed_now", installed_now.to_string())
                .human_field("previous", current.clone())
                .human_field("changed", (current != *model).to_string())
                .human_field("model_name", info.label.clone()))
            }
            ModelAction::Download(model) => {
                let existing = list_local_models(State::new(self.state.as_ref())).await?;
                let info = existing
                    .iter()
                    .find(|entry| entry.id == *model)
                    .ok_or_else(|| unknown_model_error(model, &existing))?;
                let downloaded = info.downloaded;
                if !downloaded {
                    download_local_model(State::new(self.state.as_ref()), model.clone())
                        .await
                        .map_err(|error| model_install_error(model, error))?;
                }
                Ok(CommandResult::success(
                    "model_downloaded",
                    format!("Model {model} is available"),
                )
                .field("downloaded_now", (!downloaded).to_string())
                .human_field("model", model)
                .human_field("model_name", info.label.clone()))
            }
            ModelAction::Remove(model) => {
                let models = list_local_models(State::new(self.state.as_ref())).await?;
                let info = models
                    .iter()
                    .find(|entry| entry.id == *model)
                    .ok_or_else(|| unknown_model_error(model, &models))?;
                let selected = get_transcription_settings(State::new(self.state.as_ref()))
                    .await?
                    .model;
                ensure_model_removable(&selected, model)?;
                if !info.downloaded {
                    return Ok(CommandResult::success(
                        "model_not_installed",
                        format!("Model {model} is not installed"),
                    )
                    .field("removed", "false")
                    .human_field("model", model)
                    .human_field("model_name", info.label.clone())
                    .human_field("selected", selected));
                }
                let installed_bytes = info.installed_bytes;
                let removed =
                    delete_local_model(State::new(self.state.as_ref()), model.clone()).await?;
                if !removed {
                    return Ok(CommandResult::success(
                        "model_not_installed",
                        format!("Model {model} is not installed"),
                    )
                    .field("removed", "false")
                    .human_field("model", model)
                    .human_field("model_name", info.label.clone())
                    .human_field("selected", selected));
                }
                let remaining = models
                    .iter()
                    .filter(|entry| entry.downloaded && entry.id != *model)
                    .count();
                Ok(
                    CommandResult::success("model_removed", format!("Removed model {model}"))
                        .field("removed", removed.to_string())
                        .human_field("model", model)
                        .human_field("model_name", info.label.clone())
                        .human_field(
                            "freed_bytes",
                            installed_bytes
                                .map(|bytes| bytes.to_string())
                                .unwrap_or_default(),
                        )
                        .human_field("remaining_count", remaining.to_string())
                        .human_field("selected", selected),
                )
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
                let saved_value = json_scalar(
                    json_path(&json, key).ok_or_else(|| format!("Unknown config key: {key}"))?,
                )?;
                let config: AppConfig = serde_json::from_value(json)
                    .map_err(|error| format!("Invalid value for {key}: {error}"))?;
                set_config(State::new(self.state.as_ref()), config).await?;
                Ok(
                    CommandResult::success("config_saved", format!("Saved {key}"))
                        .human_field("key", key)
                        .human_field("value", saved_value),
                )
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
                let default_value = json_scalar(&default)?;
                *json_path_mut(&mut current, key)
                    .ok_or_else(|| format!("Unknown config key: {key}"))? = default;
                let config: AppConfig = serde_json::from_value(current)
                    .map_err(|error| format!("restore default {key}: {error}"))?;
                set_config(State::new(self.state.as_ref()), config).await?;
                Ok(
                    CommandResult::success("config_reset", format!("Reset {key} to its default"))
                        .human_field("key", key)
                        .human_field("value", default_value),
                )
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
            "Cannot remove '{target}' because it is the current default model.\n\nSwitch to another installed model first:\n  origin model use <other-model-id>\n\nThen retry:\n  origin model remove {target}\n\nRun 'origin model installed' to see possible replacements."
        ));
    }
    Ok(())
}

fn unknown_model_error(requested: &str, models: &[LocalModelInfo]) -> String {
    let mut matches = models
        .iter()
        .map(|model| (edit_distance(requested, &model.id), model.id.as_str()))
        .collect::<Vec<_>>();
    matches.sort_by_key(|(distance, id)| (*distance, *id));
    let suggestions = matches
        .into_iter()
        .take(3)
        .map(|(_, id)| format!("  {id}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Model not found: {requested}\n\nClosest supported models:\n{suggestions}\n\nRun 'origin model list' to view all supported models."
    )
}

fn model_install_error(model: &str, error: String) -> String {
    format!(
        "Could not install model '{model}'. The default model was not changed.\n\nDetails: {error}\n\nCheck your network connection and available disk space, then retry:\n  origin model use {model}"
    )
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_char) in right_chars.iter().enumerate() {
            current.push(
                (current[right_index] + 1).min(
                    (previous[right_index + 1] + 1)
                        .min(previous[right_index] + usize::from(left_char != *right_char)),
                ),
            );
        }
        previous = current;
    }
    previous[right_chars.len()]
}

fn render_models(models: &[origin_speak_lib::LocalModelInfo]) -> String {
    models
        .iter()
        .map(|model| {
            let mut states = Vec::new();
            if model.selected {
                states.push("default");
            }
            if model.downloaded {
                states.push("installed");
            }
            if model.recommended {
                states.push("recommended");
            }
            let state = if states.is_empty() {
                String::new()
            } else {
                format!(" [{}]", states.join(", "))
            };
            let size = model
                .installed_bytes
                .map(|bytes| format!(" · {}", format_model_bytes(bytes)))
                .unwrap_or_default();
            format!("{}{}{}\n  {}", model.id, state, size, model.label)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| *width >= 30)
        .unwrap_or(120)
}

fn model_status(model: &LocalModelInfo) -> String {
    let mut states = Vec::new();
    if model.selected && model.downloaded {
        states.extend(["Default", "Installed"]);
    } else if model.selected {
        states.extend(["Default", "Missing"]);
    } else if model.downloaded {
        states.push("Installed");
    } else {
        states.push("Available");
    }
    if model.recommended {
        states.push("Recommended");
    }
    states.join(" · ")
}

fn render_model_catalog(models: &[LocalModelInfo], width: usize) -> String {
    let mut output = String::from("Origin Speak · Transcription Models\n\n");
    output.push_str(&render_model_table(models, width));
    let installed = models.iter().filter(|model| model.downloaded).count();
    let defaults = models.iter().filter(|model| model.selected).count();
    output.push_str(&format!(
        "\n\nModels: {} available · {installed} installed · {defaults} default\n\n",
        models.len()
    ));
    output.push_str(
        "Actions\n  Install or switch model    origin model use <model-id>\n  Install without switching  origin model install <model-id>\n  View installed models      origin model installed\n  View active model / GPU    origin model status\n  Remove installed model     origin model remove <model-id>\n\nExample\n  origin model use large-v3",
    );
    output
}

fn model_help() -> &'static str {
    "Origin Speak · Model Management\n\nUsage\n  origin model <command> [model-id]\n\nCommands\n  list                 Show every supported model and its state\n  installed            Show only models occupying local storage\n  status               Show the default model and active CPU/GPU compute\n  use <model-id>       Install if needed, then make the model the default\n  install <model-id>   Install without changing the default model\n  remove <model-id>    Remove an installed, non-default model\n\nExamples\n  origin model list\n  origin model use base.en\n  origin model install large-v3\n  origin model remove base.en\n  origin model status\n\nStable model IDs—not list row numbers—are required in commands and scripts.\nCompatibility aliases: select/switch, download, and delete/uninstall remain supported.\nUse --json anywhere in a command for machine-readable output."
}

fn render_installed_models(models: &[LocalModelInfo], width: usize) -> String {
    let mut output = String::from("Origin Speak · Installed Models\n\n");
    if models.is_empty() {
        output.push_str(
            "No transcription models are installed.\n\nInstall and select the recommended model:\n  origin model use base.en\n\nView all supported models:\n  origin model list",
        );
        return output;
    }
    output.push_str(&render_model_table(models, width));
    let total = models
        .iter()
        .filter_map(|model| model.installed_bytes)
        .sum::<u64>();
    let complete = models.iter().all(|model| model.installed_bytes.is_some());
    output.push_str(&format!("\n\nInstalled models: {}", models.len()));
    if complete {
        output.push_str(&format!(" · Total storage: {}", format_model_bytes(total)));
    }
    output.push_str("\n\nRemove a model\n  origin model remove <model-id>");
    output
}

fn render_model_table(models: &[LocalModelInfo], width: usize) -> String {
    let number_width = models.len().to_string().len().max(1);
    let id_width = models
        .iter()
        .map(|model| display_width(&model.id))
        .max()
        .unwrap_or(5)
        .max(5);
    let name_width = models
        .iter()
        .map(|model| display_width(&model.label))
        .max()
        .unwrap_or(4)
        .max(4);
    let size_width = 10;
    let status_width = models
        .iter()
        .map(|model| display_width(&model_status(model)))
        .max()
        .unwrap_or(6)
        .max(6);
    let required_width =
        1 + number_width + 2 + id_width + 2 + name_width + 2 + size_width + 2 + status_width;
    if width < 88 || required_width > width {
        let mut rows = String::from(" #  MODEL                 STATUS\n");
        rows.push_str(&format!(" {}\n", "─".repeat(width.saturating_sub(2))));
        for (index, model) in models.iter().enumerate() {
            rows.push_str(&format!(
                "{:>2}  {:<20}  {}\n    {}",
                index + 1,
                model.id,
                model_status(model),
                model.label
            ));
            if let Some(bytes) = model.installed_bytes {
                rows.push_str(&format!(" · {}", format_model_bytes(bytes)));
            }
            rows.push('\n');
        }
        return rows.trim_end().to_string();
    }

    let mut rows = format!(
        " {:>number_width$}  {}  {}  {}  STATUS\n",
        "#",
        pad_right("MODEL", id_width),
        pad_right("NAME", name_width),
        pad_right("SIZE", size_width)
    );
    rows.push_str(&format!(
        " {}\n",
        "─".repeat(required_width.saturating_sub(2))
    ));
    for (index, model) in models.iter().enumerate() {
        let size = model
            .installed_bytes
            .map(format_model_bytes)
            .unwrap_or_else(|| "—".to_string());
        rows.push_str(&format!(
            " {:>number_width$}  {}  {}  {}  {}\n",
            index + 1,
            pad_right(&model.id, id_width),
            pad_right(&model.label, name_width),
            pad_right(&size, size_width),
            model_status(model)
        ));
    }
    rows.trim_end().to_string()
}

fn pad_right(value: &str, width: usize) -> String {
    format!(
        "{value}{}",
        " ".repeat(width.saturating_sub(display_width(value)))
    )
}

fn display_width(value: &str) -> usize {
    value.chars().map(character_width).sum()
}

fn character_width(ch: char) -> usize {
    let code = ch as u32;
    if ch.is_control()
        || matches!(code, 0x0300..=0x036f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe20..=0xfe2f)
    {
        0
    } else if matches!(code, 0x1100..=0x115f | 0x2329..=0x232a | 0x2e80..=0xa4cf | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe10..=0xfe19 | 0xfe30..=0xfe6f | 0xff00..=0xff60 | 0xffe0..=0xffe6 | 0x1f300..=0x1faff | 0x20000..=0x3fffd)
    {
        2
    } else {
        1
    }
}

fn format_model_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.0} MiB", bytes as f64 / MIB)
    }
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

    fn model(
        id: &str,
        label: &str,
        downloaded: bool,
        selected: bool,
        recommended: bool,
        bytes: Option<u64>,
    ) -> LocalModelInfo {
        LocalModelInfo {
            id: id.to_string(),
            label: label.to_string(),
            filename: format!("{id}.bin"),
            revision: "test".to_string(),
            sha256: "test".to_string(),
            tier: "test".to_string(),
            english_only: true,
            recommended,
            path: format!("models/{id}.bin"),
            downloaded,
            installed_bytes: bytes,
            selected,
        }
    }

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
        assert!(error.contains("origin model use <other-model-id>"));
        assert!(error.contains("origin model remove base.en"));
        assert!(ensure_model_removable("base.en", "tiny.en").is_ok());
    }

    #[test]
    fn model_catalog_has_ordered_rows_and_unambiguous_states() {
        let models = vec![
            model("tiny.en", "Tiny English", false, false, false, None),
            model("base.en", "Base English", true, true, true, Some(1_500_000)),
            model(
                "small.en",
                "Small English",
                true,
                false,
                false,
                Some(2_500_000),
            ),
        ];
        let output = render_model_catalog(&models, 100);
        assert!(output.contains(" 1  tiny.en"));
        assert!(output.contains(" 2  base.en"));
        assert!(output.contains("Default · Installed"));
        assert!(output.contains("Installed"));
        assert!(output.contains("Models: 3 available · 2 installed · 1 default"));
        assert!(output.contains("origin model use <model-id>"));
    }

    #[test]
    fn installed_models_empty_state_is_actionable() {
        let output = render_installed_models(&[], 100);
        assert!(output.contains("No transcription models are installed"));
        assert!(output.contains("origin model use base.en"));
        assert!(output.contains("origin model list"));
    }

    #[test]
    fn installed_models_total_only_uses_known_sizes() {
        let complete = vec![
            model(
                "base.en",
                "Base English",
                true,
                true,
                false,
                Some(1024 * 1024),
            ),
            model(
                "tiny.en",
                "Tiny English",
                true,
                false,
                false,
                Some(2 * 1024 * 1024),
            ),
        ];
        assert!(render_installed_models(&complete, 100).contains("Total storage: 3 MiB"));

        let incomplete = vec![
            complete[0].clone(),
            model("tiny.en", "Tiny English", true, false, false, None),
        ];
        assert!(!render_installed_models(&incomplete, 100).contains("Total storage"));
    }

    #[test]
    fn narrow_model_table_keeps_every_row_and_name() {
        let models = vec![
            model("base.en", "Base English", false, false, true, None),
            model(
                "canary-qwen-2.5b",
                "Canary-Qwen 2.5B (Q8)",
                true,
                true,
                false,
                Some(2_797_548_928),
            ),
        ];
        let output = render_model_table(&models, 48);
        assert!(output.contains(" 1  base.en"));
        assert!(output.contains(" 2  canary-qwen-2.5b"));
        assert!(output.contains("Canary-Qwen 2.5B (Q8) · 2.61 GiB"));
    }

    #[test]
    fn table_padding_uses_terminal_cell_width_for_unicode() {
        assert_eq!(display_width("Model"), 5);
        assert_eq!(display_width("模型"), 4);
        assert_eq!(display_width("e\u{301}"), 1);
        assert_eq!(display_width(&pad_right("模型", 8)), 8);
    }

    #[test]
    fn invalid_model_error_suggests_real_ids_and_list_command() {
        let models = vec![
            model("large-v3", "Large v3", false, false, false, None),
            model(
                "large-v3-turbo",
                "Large v3 Turbo",
                false,
                false,
                false,
                None,
            ),
            model("base.en", "Base English", false, false, true, None),
        ];
        let error = unknown_model_error("large-v4", &models);
        assert!(error.contains("Model not found: large-v4"));
        assert!(error.contains("large-v3"));
        assert!(error.contains("large-v3-turbo"));
        assert!(error.contains("origin model list"));
    }

    #[test]
    fn failed_install_message_preserves_default_and_has_retry() {
        let error = model_install_error("large-v3", "network unavailable".to_string());
        assert!(error.contains("default model was not changed"));
        assert!(error.contains("network unavailable"));
        assert!(error.contains("origin model use large-v3"));
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
