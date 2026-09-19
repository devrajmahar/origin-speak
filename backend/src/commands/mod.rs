//! Dictation-only backend commands.

use crate::audio::AudioDevice;
use crate::config::LanguagePreferences;
use crate::delivery::{
    capture_surface_snapshot, strategy_chain, verify_inserted_text, DeliveryPhase,
    DeliveryStatusSnapshot, DeliveryStrategy, DeliveryUpdate,
};
use crate::{AppState, State};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Instant;

const SUPPORTED_SOURCE_LANGUAGES: &[&str] = &[
    "auto", "en", "hi", "es", "fr", "de", "it", "pt", "ru", "zh", "ja", "ko", "ar",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub text: String,
    pub duration_ms: u64,
    pub confidence: f32,
    pub is_final: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceProcessingResult {
    pub transcription: TranscriptionResult,
    pub delivered: bool,
    pub delivery_status: DeliveryStatusSnapshot,
}

#[derive(Debug, Clone)]
pub struct CapturedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

fn normalize_language_code(raw: &str) -> String {
    let mut code = raw.trim().to_lowercase();
    if code.is_empty() {
        code = "auto".to_string();
    }
    if matches!(code.as_str(), "zh-cn" | "zh-tw") {
        code = "zh".to_string();
    }
    if SUPPORTED_SOURCE_LANGUAGES.contains(&code.as_str()) {
        code
    } else {
        "auto".to_string()
    }
}

fn normalized_language_preferences(preferences: &LanguagePreferences) -> LanguagePreferences {
    LanguagePreferences {
        source_language: normalize_language_code(&preferences.source_language),
    }
}

fn is_low_signal_capture(rms: f32, peak: f32, active_ratio: f32) -> bool {
    let weak_metrics = [rms < 0.0045, peak < 0.045, active_ratio < 0.018]
        .into_iter()
        .filter(|weak| *weak)
        .count();
    weak_metrics >= 2
}

fn clean_transcription_for_delivery(text: &str) -> String {
    text.trim().to_string()
}

fn should_suppress_stock_hallucination(text: &str, rms: f32, peak: f32, active_ratio: f32) -> bool {
    const STOCK_PHRASES: &[&str] = &[
        "thank you",
        "thanks",
        "thanks for watching",
        "thank you for watching",
        "subscribe",
        "like and subscribe",
        "see you",
        "bye",
        "goodbye",
        "you",
        ".",
        "..",
        "...",
    ];

    let normalized = text
        .trim()
        .trim_matches(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '.' | ',' | '!' | '?' | ';' | ':' | '"' | '\'' | '…'
                )
        })
        .to_lowercase();
    let stock_phrase = STOCK_PHRASES.iter().any(|phrase| normalized == *phrase);
    let weak_metrics = [rms < 0.008, peak < 0.10, active_ratio < 0.05]
        .into_iter()
        .filter(|weak| *weak)
        .count();
    stock_phrase && weak_metrics >= 2
}

pub async fn start_listening(state: State<'_, AppState>) -> Result<bool, String> {
    state.transcription.ensure_ready()?;

    let mut is_listening = state.is_listening.lock().await;
    if *is_listening {
        return Ok(true);
    }

    let release_grace = {
        let streamer = state.streamer.lock().await;
        streamer.stop_streaming();
        streamer.release_grace_remaining(tokio::time::Duration::from_millis(120))
    };
    if !release_grace.is_zero() {
        tokio::time::sleep(release_grace).await;
    }

    if let Ok(mut delivery) = state.delivery.lock() {
        delivery.reset();
    }

    let preferred_device = state.audio.lock().await.selected_device.clone();
    state
        .streamer
        .lock()
        .await
        .start_streaming(preferred_device.as_deref())?;
    *is_listening = true;
    Ok(true)
}

pub async fn cancel_listening(state: State<'_, AppState>) -> Result<bool, String> {
    *state.is_listening.lock().await = false;
    {
        let streamer = state.streamer.lock().await;
        streamer.stop_streaming();
        streamer.clear_samples();
    }
    *state.is_processing.lock().await = false;
    Ok(true)
}

pub async fn take_capture_for_processing(
    state: State<'_, AppState>,
) -> Result<CapturedAudio, String> {
    if !*state.is_listening.lock().await {
        return Err("Not listening".to_string());
    }
    *state.is_listening.lock().await = false;
    state.streamer.lock().await.stop_streaming();
    *state.is_processing.lock().await = true;
    let streamer = state.streamer.lock().await;
    Ok(CapturedAudio {
        samples: streamer.get_accumulated_samples(),
        sample_rate: streamer.current_sample_rate(),
    })
}

pub async fn process_captured_audio(
    state: State<'_, AppState>,
    captured: CapturedAudio,
) -> Result<VoiceProcessingResult, String> {
    let processing_started = Instant::now();
    let CapturedAudio {
        samples,
        sample_rate,
    } = captured;

    let rms = if samples.is_empty() {
        0.0
    } else {
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
    };
    let peak = samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    let active_ratio = if samples.is_empty() {
        0.0
    } else {
        samples.iter().filter(|sample| sample.abs() > 0.012).count() as f32 / samples.len() as f32
    };
    let duration_ms = if sample_rate == 0 {
        0
    } else {
        (samples.len() as u64 * 1000) / sample_rate as u64
    };

    let minimum_samples = (sample_rate as usize / 10).max(1);
    if samples.len() < minimum_samples {
        *state.is_processing.lock().await = false;
        return Err("Recording too short.".to_string());
    }

    if is_low_signal_capture(rms, peak, active_ratio) {
        *state.is_processing.lock().await = false;
        return Ok(silent_result(duration_ms, &state));
    }

    let dictionary_hints = crate::dictionary::DictionaryStore::new()
        .and_then(|store| store.get_words_for_recognition())
        .unwrap_or_default();
    let language = {
        let config = state.config.lock().await;
        normalized_language_preferences(&config.language_preferences)
            .transcription_language_hint()
            .map(str::to_string)
    };

    let transcription_started = Instant::now();
    let text = match state
        .transcription
        .transcribe(samples, sample_rate, language, dictionary_hints)
        .await
    {
        Ok(text) => clean_transcription_for_delivery(&text),
        Err(error) => {
            state.error_log.lock().await.log_error_with_details(
                crate::error_log::ErrorType::Transcription,
                "Voice transcription failed",
                error.clone(),
            );
            *state.is_processing.lock().await = false;
            return Err(format!("Transcription failed: {error}"));
        }
    };
    let transcription_ms = transcription_started.elapsed().as_millis();

    let text_lower = text.to_lowercase();
    let repetitive_noise = {
        let words = text_lower
            .split_whitespace()
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>();
        words.len() >= 8 && words.iter().copied().collect::<HashSet<_>>().len() <= 2
    };
    if text.is_empty()
        || should_suppress_stock_hallucination(&text_lower, rms, peak, active_ratio)
        || repetitive_noise
    {
        *state.is_processing.lock().await = false;
        return Ok(silent_result(duration_ms, &state));
    }

    let transcription = TranscriptionResult {
        text: text.clone(),
        duration_ms,
        confidence: 0.0,
        is_final: true,
    };
    let delivery_started = Instant::now();
    let delivery_result = deliver_text(&state, text).await;
    let delivery_ms = delivery_started.elapsed().as_millis();
    let delivered = delivery_result.is_ok();
    if let Err(error) = delivery_result {
        state.error_log.lock().await.log_error_with_details(
            crate::error_log::ErrorType::Delivery,
            "Failed to deliver dictated text",
            error,
        );
    }

    *state.is_processing.lock().await = false;
    let result = VoiceProcessingResult {
        transcription,
        delivered,
        delivery_status: delivery_snapshot(&state),
    };
    log::info!(
        "Dictation timing: capture_ms={}, transcription_ms={}, delivery_ms={}, total_ms={}, transcript_chars={}",
        duration_ms,
        transcription_ms,
        delivery_ms,
        processing_started.elapsed().as_millis(),
        result.transcription.text.chars().count()
    );
    Ok(result)
}

fn silent_result(duration_ms: u64, state: &State<'_, AppState>) -> VoiceProcessingResult {
    VoiceProcessingResult {
        transcription: TranscriptionResult {
            text: String::new(),
            duration_ms,
            confidence: 0.0,
            is_final: true,
        },
        delivered: false,
        delivery_status: delivery_snapshot(state),
    }
}

fn delivery_snapshot(state: &State<'_, AppState>) -> DeliveryStatusSnapshot {
    state
        .delivery
        .lock()
        .map(|delivery| delivery.snapshot())
        .unwrap_or_default()
}

pub async fn get_audio_level(state: State<'_, AppState>) -> Result<f32, String> {
    if !*state.is_listening.lock().await {
        return Ok(0.0);
    }
    Ok(state.streamer.lock().await.get_live_level())
}

pub async fn get_audio_devices() -> Result<Vec<AudioDevice>, String> {
    crate::audio::AudioState::get_devices()
}

fn is_handsfree_input_name(name: &str) -> bool {
    let normalized = name.to_lowercase();
    normalized.contains("hands-free")
        || normalized.contains("hands free")
        || normalized.contains("ag audio")
        || normalized.contains("hfp")
        || normalized.contains("hsp")
}

pub async fn set_audio_device(
    state: State<'_, AppState>,
    device_name: String,
) -> Result<bool, String> {
    let cleaned_name = device_name.trim();
    if cleaned_name.is_empty() {
        return Err("Audio device name cannot be empty".to_string());
    }
    if is_handsfree_input_name(cleaned_name) {
        return Err(
            "Bluetooth hands-free microphones are blocked because they can hijack headphone output audio."
                .to_string(),
        );
    }
    if !crate::audio::AudioState::get_devices()?
        .iter()
        .any(|device| device.name == cleaned_name)
    {
        return Err(format!(
            "Audio input device is no longer available: {cleaned_name}"
        ));
    }

    let _guard = state.config_update_lock.lock().await;
    let mut config = state.config.lock().await.clone();
    config.selected_audio_device = Some(cleaned_name.to_string());
    commit_config(&state, config).await?;
    Ok(true)
}

pub async fn list_local_models(
    state: State<'_, AppState>,
) -> Result<Vec<crate::LocalModelInfo>, String> {
    state.transcription.list_models()
}

pub async fn get_transcription_settings(
    state: State<'_, AppState>,
) -> Result<crate::TranscriptionSettings, String> {
    Ok(state.transcription.settings())
}

pub async fn set_transcription_model(
    state: State<'_, AppState>,
    model: String,
) -> Result<crate::TranscriptionSettings, String> {
    state.transcription.set_model(&model)
}

pub async fn get_transcription_runtime_status(
    state: State<'_, AppState>,
) -> Result<crate::TranscriptionRuntimeStatus, String> {
    Ok(state.transcription.runtime_status())
}

pub async fn download_local_model(
    state: State<'_, AppState>,
    model: String,
) -> Result<crate::LocalModelInfo, String> {
    state.transcription.download_model(&model).await
}

pub async fn delete_local_model(state: State<'_, AppState>, model: String) -> Result<bool, String> {
    state.transcription.remove_model(&model).await
}

pub async fn get_config(state: State<'_, AppState>) -> Result<crate::AppConfig, String> {
    Ok(state.config.lock().await.clone())
}

pub async fn set_config(
    state: State<'_, AppState>,
    config: crate::AppConfig,
) -> Result<bool, String> {
    let _guard = state.config_update_lock.lock().await;
    commit_config(&state, config).await
}

async fn commit_config(state: &AppState, mut config: crate::AppConfig) -> Result<bool, String> {
    config.trigger_hotkey = normalize_hotkey_string(&config.trigger_hotkey)?;
    config.language_preferences = normalized_language_preferences(&config.language_preferences);
    let old_use_gpu = state.config.lock().await.use_gpu;
    config.save_to_disk()?;
    *state.config.lock().await = config.clone();
    state.audio.lock().await.selected_device = config.selected_audio_device.clone();
    if old_use_gpu != config.use_gpu {
        state.transcription.set_gpu_enabled(config.use_gpu);
    }
    Ok(true)
}

pub fn normalize_hotkey_string(raw: &str) -> Result<String, String> {
    let cleaned = raw.trim();
    if cleaned.is_empty() {
        return Err("Hotkey cannot be empty".to_string());
    }
    let mut modifiers = Vec::<String>::new();
    let mut key = None::<String>;
    for part in cleaned.split('+') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        let lower = token.to_lowercase();
        let modifier = match lower.as_str() {
            "ctrl" | "control" => Some("Ctrl"),
            "alt" | "option" => Some("Alt"),
            "shift" => Some("Shift"),
            "win" | "windows" | "meta" | "super" | "cmd" | "command" => Some("Super"),
            _ => None,
        };
        if let Some(modifier) = modifier {
            if !modifiers.iter().any(|existing| existing == modifier) {
                modifiers.push(modifier.to_string());
            }
            continue;
        }
        if key.is_none() {
            key = Some(if matches!(lower.as_str(), "space" | "spacebar") {
                "Space".to_string()
            } else {
                let mut chars = token.chars();
                let first = chars
                    .next()
                    .ok_or_else(|| "Hotkey must include a non-modifier key".to_string())?;
                format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase())
            });
        }
    }
    let key = key.ok_or_else(|| "Hotkey must include a non-modifier key".to_string())?;
    if modifiers.is_empty() {
        return Err("Hotkey must include at least one modifier key".to_string());
    }
    if key == "Space"
        && modifiers.len() == 2
        && modifiers.iter().any(|modifier| modifier == "Super")
        && modifiers.iter().any(|modifier| modifier == "Ctrl")
    {
        return Ok("Shift+Space".to_string());
    }
    modifiers.push(key);
    let normalized = modifiers.join("+");
    normalized
        .parse::<global_hotkey::hotkey::HotKey>()
        .map_err(|error| format!("Unsupported global hotkey '{normalized}': {error}"))?;
    Ok(normalized)
}

pub async fn get_trigger_hotkey(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.config.lock().await.trigger_hotkey.clone())
}

pub async fn set_trigger_hotkey(
    state: State<'_, AppState>,
    hotkey: String,
) -> Result<String, String> {
    let normalized = normalize_hotkey_string(&hotkey)?;
    let _guard = state.config_update_lock.lock().await;
    let mut config = state.config.lock().await.clone();
    config.trigger_hotkey = normalized.clone();
    commit_config(&state, config).await?;
    Ok(normalized)
}

pub async fn get_language_preferences(
    state: State<'_, AppState>,
) -> Result<LanguagePreferences, String> {
    Ok(normalized_language_preferences(
        &state.config.lock().await.language_preferences,
    ))
}

pub async fn set_language_preferences(
    state: State<'_, AppState>,
    source_language: String,
) -> Result<LanguagePreferences, String> {
    let normalized = LanguagePreferences {
        source_language: normalize_language_code(&source_language),
    };
    let _guard = state.config_update_lock.lock().await;
    let mut config = state.config.lock().await.clone();
    config.language_preferences = normalized.clone();
    commit_config(&state, config).await?;
    Ok(normalized)
}

pub async fn get_dictionary_words() -> Result<Vec<crate::DictionaryWord>, String> {
    crate::dictionary::DictionaryStore::new()?.get_all_words()
}

pub async fn add_dictionary_word(
    word: String,
    is_auto_learned: bool,
) -> Result<crate::DictionaryWord, String> {
    crate::dictionary::DictionaryStore::new()?.add_word(word, is_auto_learned)
}

pub async fn update_dictionary_word(
    id: String,
    word: String,
    phonetic: Option<String>,
) -> Result<(), String> {
    crate::dictionary::DictionaryStore::new()?.update_word(&id, word, phonetic)
}

pub async fn delete_dictionary_word(id: String) -> Result<(), String> {
    crate::dictionary::DictionaryStore::new()?.delete_word(&id)
}

fn update_delivery_state(
    state: &State<'_, AppState>,
    update: impl FnOnce(&mut crate::delivery::DeliveryState),
) {
    if let Ok(mut delivery) = state.delivery.lock() {
        update(&mut delivery);
    }
}

fn restore_previous_clipboard(
    clipboard: &mut arboard::Clipboard,
    previous_content: Option<&String>,
) {
    if let Some(previous_content) = previous_content {
        let _ = clipboard.set_text(previous_content);
    }
}

fn set_clipboard_text(clipboard: &mut arboard::Clipboard, text: &str) -> Result<(), String> {
    for attempt in 1..=3 {
        match clipboard.set_text(text) {
            Ok(_) => {
                std::thread::sleep(std::time::Duration::from_millis(8));
                if clipboard
                    .get_text()
                    .map(|current| current == text)
                    .unwrap_or(false)
                {
                    return Ok(());
                }
            }
            Err(error) => log::warn!("Clipboard set failed on attempt {attempt}: {error}"),
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Err("Failed to set clipboard for text delivery".to_string())
}

fn perform_delivery_strategy(strategy: DeliveryStrategy, text: &str) -> Result<(), String> {
    use enigo::{Enigo, Key, Keyboard, Settings};
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|error| format!("Failed to create input injector: {error}"))?;
    match strategy {
        DeliveryStrategy::CtrlV => {
            #[cfg(target_os = "macos")]
            let modifiers = [Key::Meta];
            #[cfg(not(target_os = "macos"))]
            let modifiers = [Key::Control];
            send_hotkey(&mut enigo, &modifiers, Key::Unicode('v'))
        }
        DeliveryStrategy::CtrlShiftV => {
            #[cfg(target_os = "macos")]
            let modifiers = [Key::Meta, Key::Shift];
            #[cfg(not(target_os = "macos"))]
            let modifiers = [Key::Control, Key::Shift];
            send_hotkey(&mut enigo, &modifiers, Key::Unicode('v'))
        }
        DeliveryStrategy::ShiftInsert => {
            #[cfg(target_os = "macos")]
            {
                Err("Shift+Insert delivery is not supported on macOS".to_string())
            }
            #[cfg(not(target_os = "macos"))]
            {
                send_hotkey(&mut enigo, &[Key::Shift], Key::Insert)
            }
        }
        DeliveryStrategy::SimulatedTyping => enigo
            .text(text)
            .map_err(|error| format!("Failed to simulate typing: {error}")),
    }
}

fn send_hotkey(
    enigo: &mut enigo::Enigo,
    modifiers: &[enigo::Key],
    final_key: enigo::Key,
) -> Result<(), String> {
    use enigo::{Direction, Keyboard};
    for modifier in modifiers {
        enigo
            .key(*modifier, Direction::Press)
            .map_err(|error| format!("Failed to press modifier key: {error}"))?;
        std::thread::sleep(std::time::Duration::from_millis(14));
    }
    enigo
        .key(final_key, Direction::Click)
        .map_err(|error| format!("Failed to press delivery key: {error}"))?;
    std::thread::sleep(std::time::Duration::from_millis(18));
    for modifier in modifiers.iter().rev() {
        enigo
            .key(*modifier, Direction::Release)
            .map_err(|error| format!("Failed to release modifier key: {error}"))?;
    }
    Ok(())
}

async fn deliver_text(state: &State<'_, AppState>, text: String) -> Result<(), String> {
    use arboard::Clipboard;
    if text.trim().is_empty() {
        return Err("No text to type".to_string());
    }

    update_delivery_state(state, |delivery| delivery.begin(&text));
    tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;

    let before = capture_surface_snapshot(4096);
    let surface = before.classify();
    let target_label = before
        .target_label
        .clone()
        .or_else(|| before.window_title.clone())
        .or_else(|| before.process_name.clone());
    let target_name = target_label
        .clone()
        .unwrap_or_else(|| "focused application".to_string());
    update_delivery_state(state, |delivery| {
        delivery.update(DeliveryUpdate {
            phase: DeliveryPhase::Preparing,
            surface,
            target: target_label.clone(),
            strategy: None,
            attempts: 0,
            summary: format!("Targeting {target_name}"),
            recovered_to_clipboard: false,
        });
    });

    let strategies = strategy_chain(surface, &before, &text);
    let mut clipboard = Clipboard::new().ok();
    let previous_clipboard = clipboard
        .as_mut()
        .and_then(|clipboard| clipboard.get_text().ok());
    let mut last_error = None::<String>;
    let mut attempted_strategy = None::<DeliveryStrategy>;
    let mut attempts_made = 0_u8;

    for (index, strategy) in strategies.iter().enumerate() {
        let attempt = (index + 1) as u8;
        update_delivery_state(state, |delivery| {
            delivery.update(DeliveryUpdate {
                phase: if attempt > 1 {
                    DeliveryPhase::Retrying
                } else {
                    DeliveryPhase::Injecting
                },
                surface,
                target: target_label.clone(),
                strategy: Some(*strategy),
                attempts: attempt,
                summary: format!("Trying {} for {target_name}", strategy.label()),
                recovered_to_clipboard: false,
            });
        });

        if !matches!(strategy, DeliveryStrategy::SimulatedTyping) {
            let Some(clipboard) = clipboard.as_mut() else {
                last_error = Some("Clipboard is unavailable for paste delivery".to_string());
                continue;
            };
            if let Err(error) = set_clipboard_text(clipboard, &text) {
                last_error = Some(error);
                continue;
            }
        }

        attempted_strategy = Some(*strategy);
        attempts_made = attempts_made.saturating_add(1);
        if let Err(error) = perform_delivery_strategy(*strategy, &text) {
            last_error = Some(error);
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        update_delivery_state(state, |delivery| {
            delivery.update(DeliveryUpdate {
                phase: DeliveryPhase::Verifying,
                surface,
                target: target_label.clone(),
                strategy: Some(*strategy),
                attempts: attempt,
                summary: format!("Verifying {} delivery", strategy.label()),
                recovered_to_clipboard: false,
            });
        });

        let after = capture_surface_snapshot(4096);
        let verified = verify_inserted_text(&before, &after, &text);
        let readback_unavailable = !before.supports_readback() || !after.supports_readback();
        if let Some(clipboard) = clipboard.as_mut() {
            restore_previous_clipboard(clipboard, previous_clipboard.as_ref());
        }
        let message = if verified {
            format!("Delivered text to {target_name} via {}", strategy.label())
        } else if readback_unavailable {
            format!(
                "Delivered text to {target_name} via {} (readback unavailable)",
                strategy.label()
            )
        } else {
            format!(
                "Delivered text to {target_name} via {} (readback inconclusive)",
                strategy.label()
            )
        };
        update_delivery_state(state, |delivery| {
            delivery.update(DeliveryUpdate {
                phase: DeliveryPhase::Succeeded,
                surface,
                target: target_label.clone(),
                strategy: Some(*strategy),
                attempts: attempt,
                summary: message.clone(),
                recovered_to_clipboard: false,
            });
        });
        return Ok(());
    }

    let recovered_to_clipboard = clipboard
        .as_mut()
        .map(|clipboard| clipboard.set_text(&text).is_ok())
        .unwrap_or(false);
    let failure_message = if recovered_to_clipboard {
        format!(
            "Failed to deliver text to {target_name}. Transcript was copied to the clipboard for recovery."
        )
    } else {
        format!(
            "Failed to deliver text to {target_name}. Transcript was kept in the recovery buffer."
        )
    };
    update_delivery_state(state, |delivery| {
        delivery.store_failure(
            text.clone(),
            failure_message.clone(),
            recovered_to_clipboard,
        );
        delivery.update(DeliveryUpdate {
            phase: DeliveryPhase::RecoverableFailure,
            surface,
            target: target_label,
            strategy: attempted_strategy,
            attempts: attempts_made,
            summary: failure_message.clone(),
            recovered_to_clipboard,
        });
    });
    Err(match last_error {
        Some(last_error) => format!("{failure_message} {last_error}"),
        None => failure_message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_normalization_is_single_binding_only() {
        assert_eq!(
            normalize_hotkey_string(" control + shift + spacebar ").unwrap(),
            "Ctrl+Shift+Space"
        );
        assert_eq!(normalize_hotkey_string("cmd+K").unwrap(), "Super+K");
        assert_eq!(
            normalize_hotkey_string("meta+ctrl+space").unwrap(),
            "Shift+Space"
        );
        assert_eq!(
            normalize_hotkey_string("ctrl+win+space").unwrap(),
            "Shift+Space"
        );
        assert_eq!(
            normalize_hotkey_string("win+shift+k").unwrap(),
            "Super+Shift+K"
        );
        assert_eq!(
            normalize_hotkey_string("Shift+Space").unwrap(),
            "Shift+Space"
        );
        assert!(normalize_hotkey_string("ctrl + win").is_err());
    }

    #[test]
    fn dictation_cleanup_only_trims_outer_whitespace() {
        assert_eq!(
            clean_transcription_for_delivery("  open settings and delete nothing, please.  \n"),
            "open settings and delete nothing, please."
        );
    }

    #[test]
    fn stock_whisper_phrases_are_only_suppressed_for_weak_audio() {
        assert!(should_suppress_stock_hallucination(
            "thank you",
            0.001,
            0.01,
            0.002
        ));
        assert!(!should_suppress_stock_hallucination(
            "thank you",
            0.025,
            0.22,
            0.15
        ));
    }

    #[test]
    fn silence_gate_tolerates_single_noise_spike() {
        assert!(is_low_signal_capture(0.003, 0.12, 0.006));
        assert!(!is_low_signal_capture(0.007, 0.11, 0.05));
    }
}
