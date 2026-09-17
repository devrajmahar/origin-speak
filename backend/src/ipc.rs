use crate::{commands, AppState, State};
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::Write,
    str::FromStr,
    sync::{Arc, RwLock},
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

#[derive(Debug, Deserialize)]
struct Request {
    id: u64,
    command: String,
    #[serde(default)]
    args: Value,
}

#[derive(Clone, Copy, Default)]
struct ShortcutIds {
    trigger: u32,
    assistant: u32,
}

struct ShortcutService {
    manager: GlobalHotKeyManager,
    trigger: HotKey,
    assistant: HotKey,
    ids: Arc<RwLock<ShortcutIds>>,
}

impl ShortcutService {
    fn new(
        trigger: &str,
        assistant: &str,
        outbound: mpsc::UnboundedSender<Value>,
    ) -> Result<Self, String> {
        let manager = GlobalHotKeyManager::new().map_err(|error| error.to_string())?;
        let trigger = HotKey::from_str(trigger).map_err(|error| error.to_string())?;
        let assistant = HotKey::from_str(assistant).map_err(|error| error.to_string())?;

        manager
            .register(trigger.clone())
            .map_err(|error| error.to_string())?;
        manager
            .register(assistant.clone())
            .map_err(|error| error.to_string())?;

        let ids = Arc::new(RwLock::new(ShortcutIds {
            trigger: trigger.id(),
            assistant: assistant.id(),
        }));
        let event_ids = ids.clone();

        std::thread::spawn(move || {
            while let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
                let current = event_ids.read().map(|guard| *guard).unwrap_or_default();
                let event_name = if event.id == current.trigger {
                    match event.state {
                        HotKeyState::Pressed => Some("shortcut-pressed"),
                        HotKeyState::Released => Some("shortcut-released"),
                    }
                } else if event.id == current.assistant && event.state == HotKeyState::Pressed {
                    Some("assistant-shortcut")
                } else {
                    None
                };

                if let Some(event_name) = event_name {
                    let _ = outbound.send(json!({ "event": event_name, "payload": null }));
                }
            }
        });

        Ok(Self {
            manager,
            trigger,
            assistant,
            ids,
        })
    }

    fn update(&mut self, trigger: &str, assistant: &str) -> Result<(), String> {
        let next_trigger = HotKey::from_str(trigger).map_err(|error| error.to_string())?;
        let next_assistant = HotKey::from_str(assistant).map_err(|error| error.to_string())?;

        self.manager
            .unregister(self.trigger.clone())
            .map_err(|error| error.to_string())?;
        if let Err(error) = self.manager.unregister(self.assistant.clone()) {
            let _ = self.manager.register(self.trigger.clone());
            return Err(error.to_string());
        }

        if let Err(error) = self.manager.register(next_trigger.clone()) {
            let _ = self.manager.register(self.trigger.clone());
            let _ = self.manager.register(self.assistant.clone());
            return Err(error.to_string());
        }
        if let Err(error) = self.manager.register(next_assistant.clone()) {
            let _ = self.manager.unregister(next_trigger.clone());
            let _ = self.manager.register(self.trigger.clone());
            let _ = self.manager.register(self.assistant.clone());
            return Err(error.to_string());
        }

        self.trigger = next_trigger;
        self.assistant = next_assistant;
        if let Ok(mut ids) = self.ids.write() {
            *ids = ShortcutIds {
                trigger: next_trigger.id(),
                assistant: next_assistant.id(),
            };
        }
        Ok(())
    }
}

pub async fn run() {
    let state = AppState::default();
    let (outbound, mut outgoing) = mpsc::unbounded_channel::<Value>();

    std::thread::spawn(move || {
        let stdout = std::io::stdout();
        let mut stdout = std::io::BufWriter::new(stdout.lock());
        while let Some(message) = outgoing.blocking_recv() {
            if serde_json::to_writer(&mut stdout, &message).is_err() {
                break;
            }
            if stdout.write_all(b"\n").is_err() || stdout.flush().is_err() {
                break;
            }
        }
    });

    let (trigger, assistant) = {
        let config = state.config.lock().await;
        (
            config.trigger_hotkey.clone(),
            config.assistant_hotkey.clone(),
        )
    };
    let mut shortcuts = match ShortcutService::new(&trigger, &assistant, outbound.clone()) {
        Ok(service) => Some(service),
        Err(error) => {
            log::error!("Global shortcuts unavailable: {error}");
            let _ = outbound.send(json!({
                "event": "backend-warning",
                "payload": { "message": format!("Global shortcuts unavailable: {error}") }
            }));
            None
        }
    };

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                let _ = outbound.send(json!({ "id": null, "error": error.to_string() }));
                continue;
            }
        };

        // Model downloads may take minutes. Run them independently so the stdin
        // loop can continue servicing capture stop/cancel, status, and settings
        // requests while bytes are streaming to disk.
        if request.command == "download_local_model" {
            let model = match required::<String>(&request.args, &["model"]) {
                Ok(model) => model,
                Err(error) => {
                    let _ = outbound.send(json!({ "id": request.id, "error": error }));
                    continue;
                }
            };
            let transcription = state.transcription.clone();
            let response_outbound = outbound.clone();
            tokio::spawn(async move {
                let result = transcription.download_model(&model).await.and_then(value);
                match result {
                    Ok(result) => {
                        let _ =
                            response_outbound.send(json!({ "id": request.id, "result": result }));
                    }
                    Err(error) => {
                        let _ = response_outbound.send(json!({ "id": request.id, "error": error }));
                    }
                }
            });
            continue;
        }

        let updates_hotkeys = matches!(
            request.command.as_str(),
            "set_config" | "set_trigger_hotkey" | "set_assistant_hotkey"
        );
        let previous_hotkeys = if updates_hotkeys {
            let config = state.config.lock().await;
            Some((
                config.trigger_hotkey.clone(),
                config.assistant_hotkey.clone(),
            ))
        } else {
            None
        };

        let mut result = dispatch(&state, &request.command, &request.args).await;
        if result.is_ok() && updates_hotkeys {
            let (trigger, assistant) = {
                let config = state.config.lock().await;
                (
                    config.trigger_hotkey.clone(),
                    config.assistant_hotkey.clone(),
                )
            };
            if let Some(service) = shortcuts.as_mut() {
                if let Err(error) = service.update(&trigger, &assistant) {
                    log::error!("Failed to update global shortcuts: {error}");
                    result = Err(format!("Failed to update global shortcuts: {error}"));
                }
            } else {
                match ShortcutService::new(&trigger, &assistant, outbound.clone()) {
                    Ok(service) => shortcuts = Some(service),
                    Err(error) => {
                        result = Err(format!("Failed to register global shortcuts: {error}"));
                    }
                }
            }

            if result.is_err() {
                if let Some((trigger, assistant)) = previous_hotkeys {
                    let mut config = state.config.lock().await;
                    config.trigger_hotkey = trigger;
                    config.assistant_hotkey = assistant;
                }
            }
        }

        match result {
            Ok(value) => {
                if request.command == "learn_correction" {
                    if let Some(words) = value.as_array() {
                        for word in words.iter().filter_map(Value::as_str) {
                            let _ = outbound.send(json!({
                                "event": "word-learned",
                                "payload": { "word": word }
                            }));
                        }
                    }
                }
                let _ = outbound.send(json!({ "id": request.id, "result": value }));
            }
            Err(error) => {
                let _ = outbound.send(json!({ "id": request.id, "error": error }));
            }
        }
    }

    drop(outbound);
}

async fn dispatch(state: &AppState, command: &str, args: &Value) -> Result<Value, String> {
    let state = State::new(state);
    match command {
        "start_listening" => value(commands::start_listening(state).await?),
        "cancel_listening" => value(commands::cancel_listening(state).await?),
        "stop_listening" => value(
            commands::stop_listening(state, optional(args, &["dictationOnly", "dictation_only"])?)
                .await?,
        ),
        "get_status" => value(commands::get_status(state).await?),
        "get_audio_level" => value(commands::get_audio_level(state).await?),
        "get_pending_action" => value(commands::get_pending_action(state).await?),
        "confirm_pending_action" => value(commands::confirm_pending_action(state).await?),
        "cancel_pending_action" => value(commands::cancel_pending_action(state).await?),
        "get_audio_devices" => value(commands::get_audio_devices().await?),
        "set_audio_device" => value(
            commands::set_audio_device(state, required(args, &["deviceName", "device_name"])?)
                .await?,
        ),
        "list_local_models" => value(commands::list_local_models(state).await?),
        "get_transcription_settings" => value(commands::get_transcription_settings(state).await?),
        "set_transcription_model" => {
            value(commands::set_transcription_model(state, required(args, &["model"])?).await?)
        }
        "get_transcription_runtime_status" => {
            value(commands::get_transcription_runtime_status(state).await?)
        }
        "download_local_model" => {
            value(commands::download_local_model(state, required(args, &["model"])?).await?)
        }
        "type_text" => value(commands::type_text(state, required(args, &["text"])?).await?),
        "run_system_command" => {
            value(commands::run_system_command(required(args, &["command"])?).await?)
        }
        "get_conversation" => value(commands::get_conversation(state).await?),
        "clear_conversation" => value(commands::clear_conversation(state).await?),
        "new_conversation_session" => value(commands::new_conversation_session(state).await?),
        "get_clipboard" => value(commands::get_clipboard(state).await?),
        "set_clipboard" => {
            value(commands::set_clipboard(state, required(args, &["content"])?).await?)
        }
        "get_clipboard_history" => {
            value(commands::get_clipboard_history(state, optional(args, &["limit"])?).await?)
        }
        "get_integrations" => value(commands::get_integrations(state).await?),
        "set_integration_enabled" => value(
            commands::set_integration_enabled(
                state,
                required(args, &["name"])?,
                required(args, &["enabled"])?,
            )
            .await?,
        ),
        "get_trigger_hotkey" => value(commands::get_trigger_hotkey(state).await?),
        "set_trigger_hotkey" => {
            value(commands::set_trigger_hotkey(state, required(args, &["hotkey"])?).await?)
        }
        "get_assistant_hotkey" => value(commands::get_assistant_hotkey(state).await?),
        "set_assistant_hotkey" => {
            value(commands::set_assistant_hotkey(state, required(args, &["hotkey"])?).await?)
        }
        "get_language_preferences" => value(commands::get_language_preferences(state).await?),
        "set_language_preferences" => value(
            commands::set_language_preferences(
                state,
                required(args, &["sourceLanguage", "source_language"])?,
                required(args, &["targetLanguage", "target_language"])?,
            )
            .await?,
        ),
        "get_vibe_coding_config" => value(commands::get_vibe_coding_config(state).await?),
        "set_vibe_coding_config" => {
            value(commands::set_vibe_coding_config(state, required(args, &["config"])?).await?)
        }
        "get_custom_commands" => value(commands::get_custom_commands().await?),
        "get_command_templates" => value(commands::get_command_templates().await?),
        "save_custom_command" => {
            value(commands::save_custom_command(required(args, &["command"])?).await?)
        }
        "delete_custom_command" => {
            value(commands::delete_custom_command(required(args, &["id"])?).await?)
        }
        "set_custom_command_enabled" => value(
            commands::set_custom_command_enabled(
                required(args, &["id"])?,
                required(args, &["enabled"])?,
            )
            .await?,
        ),
        "export_custom_commands" => value(commands::export_custom_commands().await?),
        "import_custom_commands" => {
            value(commands::import_custom_commands(required(args, &["json"])?).await?)
        }
        "get_history" => {
            let history = state.history.lock().await;
            value(history.clone())
        }
        "clear_history" => {
            state.history.lock().await.clear();
            value(())
        }
        "get_notes" => value(crate::NotesStore::new()?.get_all_notes(optional(args, &["limit"])?)?),
        "create_note" => {
            value(crate::NotesStore::new()?.create_note(required(args, &["content"])?)?)
        }
        "update_note" => value(crate::NotesStore::new()?.update_note(
            &required::<String>(args, &["id"])?,
            required(args, &["content"])?,
        )?),
        "delete_note" => {
            value(crate::NotesStore::new()?.delete_note(&required::<String>(args, &["id"])?)?)
        }
        "toggle_note_pin" => {
            value(crate::NotesStore::new()?.toggle_pin(&required::<String>(args, &["id"])?)?)
        }
        "create_voice_note" => create_voice_note(state).await.and_then(value),
        "get_snippets" => value(crate::SnippetsStore::new()?.get_all_snippets()?),
        "create_snippet" => value(crate::SnippetsStore::new()?.create_snippet(
            required(args, &["trigger"])?,
            required(args, &["expansion"])?,
        )?),
        "update_snippet" => value(crate::SnippetsStore::new()?.update_snippet(
            &required::<String>(args, &["id"])?,
            required(args, &["trigger"])?,
            required(args, &["expansion"])?,
        )?),
        "delete_snippet" => {
            value(crate::SnippetsStore::new()?.delete_snippet(&required::<String>(args, &["id"])?)?)
        }
        "get_dictionary_words" => value(crate::DictionaryStore::new()?.get_all_words()?),
        "add_dictionary_word" => value(crate::DictionaryStore::new()?.add_word(
            required(args, &["word"])?,
            optional(args, &["isAutoLearned", "is_auto_learned"])?.unwrap_or(false),
        )?),
        "update_dictionary_word" => value(crate::DictionaryStore::new()?.update_word(
            &required::<String>(args, &["id"])?,
            required(args, &["word"])?,
            optional(args, &["phonetic"])?,
        )?),
        "delete_dictionary_word" => {
            value(crate::DictionaryStore::new()?.delete_word(&required::<String>(args, &["id"])?)?)
        }
        "get_errors" => {
            let error_log = state.error_log.lock().await;
            value(error_log.get_recent(optional(args, &["limit"])?.unwrap_or(20)))
        }
        "get_undismissed_errors" => {
            let error_log = state.error_log.lock().await;
            value(error_log.get_undismissed())
        }
        "dismiss_error" => {
            let mut error_log = state.error_log.lock().await;
            value(error_log.dismiss(&required::<String>(args, &["id"])?))
        }
        "dismiss_all_errors" => {
            state.error_log.lock().await.dismiss_all();
            value(())
        }
        "learn_correction" => {
            learn_correction(state, required(args, &["correctedText", "corrected_text"])?).await
        }
        _ => Err(format!("Unknown native command: {command}")),
    }
}

async fn create_voice_note(state: State<'_, AppState>) -> Result<crate::Note, String> {
    let (samples, sample_rate) = {
        let streamer = state.streamer.lock().await;
        (
            streamer.get_accumulated_samples(),
            streamer.current_sample_rate(),
        )
    };
    let minimum_samples = (sample_rate as usize / 10).max(1);
    if samples.len() < minimum_samples {
        return Err("Recording too short".to_string());
    }

    let rms =
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt();
    let peak = samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    let active_ratio =
        samples.iter().filter(|sample| sample.abs() > 0.012).count() as f32 / samples.len() as f32;
    if rms < 0.0028 && peak < 0.02 && active_ratio < 0.01 {
        return Err("No speech detected".to_string());
    }

    let (language, dictionary_hints) = {
        let config = state.config.lock().await;
        let language = config
            .language_preferences
            .transcription_language_hint()
            .map(str::to_string);
        let dictionary_hints = crate::DictionaryStore::new()
            .and_then(|store| store.get_words_for_recognition())
            .unwrap_or_default();
        (language, dictionary_hints)
    };

    let result = state
        .transcription
        .transcribe(samples, sample_rate, language, dictionary_hints)
        .await?;
    let text = result.trim();
    if text.is_empty() {
        return Err("No speech detected".to_string());
    }
    crate::NotesStore::new()?.create_note(text.to_string())
}

async fn learn_correction(
    state: State<'_, AppState>,
    corrected_text: String,
) -> Result<Value, String> {
    let corrections = state
        .correction_tracker
        .lock()
        .await
        .detect_corrections(&corrected_text);
    let store = crate::DictionaryStore::new()?;
    let mut learned = Vec::new();
    for (_, corrected) in corrections {
        if !store.word_exists(&corrected)? {
            store.add_word(corrected.clone(), true)?;
            learned.push(corrected);
        }
    }
    value(learned)
}

fn required<T: DeserializeOwned>(args: &Value, names: &[&str]) -> Result<T, String> {
    let value = names
        .iter()
        .find_map(|name| args.get(*name))
        .ok_or_else(|| format!("Missing argument: {}", names[0]))?;
    serde_json::from_value(value.clone()).map_err(|error| error.to_string())
}

fn optional<T: DeserializeOwned>(args: &Value, names: &[&str]) -> Result<Option<T>, String> {
    match names.iter().find_map(|name| args.get(*name)) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| error.to_string()),
    }
}

fn value<T: Serialize>(result: T) -> Result<Value, String> {
    serde_json::to_value(result).map_err(|error| error.to_string())
}
