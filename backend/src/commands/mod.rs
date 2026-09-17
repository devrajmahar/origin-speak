//! Desktop command handlers for ListenOS.
//!
//! Native voice command and dictation handling.

pub mod custom;

use crate::audio::AudioDevice;
use crate::config::{
    LanguagePreferences, VibeActivationMode, VibeCodingConfig, VibeFormattingStyle,
};
use crate::delivery::{
    capture_surface_snapshot, strategy_chain, verify_inserted_text, DeliveryPhase,
    DeliveryStatusSnapshot, DeliveryStrategy, DeliveryUpdate,
};
use crate::voice::{
    self, ActionResult, ActionType, ConversationContext, VoiceContext, VoiceMode, VoiceRouter,
};
use crate::{AppState, State};
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Status response for frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    pub is_listening: bool,
    pub is_processing: bool,
    pub is_streaming: bool,
    pub audio_device: Option<String>,
    pub last_transcription: Option<String>,
    pub audio_status: crate::AudioRuntimeStatus,
    pub delivery_status: DeliveryStatusSnapshot,
}

/// Transcription result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub text: String,
    pub duration_ms: u64,
    pub confidence: f32,
    pub is_final: bool,
}

/// Command result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub success: bool,
    pub message: String,
    pub output: Option<String>,
}

/// Full voice processing result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceProcessingResult {
    pub transcription: TranscriptionResult,
    pub action: ActionResultResponse,
    pub executed: bool,
    /// AI response text for conversational actions
    pub response_text: Option<String>,
    /// Session ID for conversation continuity
    pub session_id: String,
    pub delivery_status: DeliveryStatusSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResultResponse {
    pub action_type: String,
    pub payload: serde_json::Value,
    pub refined_text: Option<String>,
    pub response_text: Option<String>,
    pub requires_confirmation: bool,
    pub pending_action_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingAction {
    pub id: String,
    pub action: ActionResult,
    pub transcription: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingActionResponse {
    pub id: String,
    pub action_type: String,
    pub payload: serde_json::Value,
    pub transcription: String,
    pub summary: String,
    pub created_at: String,
}

const SUPPORTED_SOURCE_LANGUAGES: &[&str] = &[
    "auto", "en", "hi", "es", "fr", "de", "it", "pt", "ru", "zh", "ja", "ko", "ar",
];
const SUPPORTED_TARGET_LANGUAGES: &[&str] = &[
    "en", "hi", "es", "fr", "de", "it", "pt", "ru", "zh", "ja", "ko", "ar",
];

#[derive(Debug, Clone)]
struct MultilingualTextResult {
    routing_text: String,
    output_text: String,
    transformed: bool,
}

fn normalize_language_code(raw: &str, allow_auto: bool) -> String {
    let mut code = raw.trim().to_lowercase();
    if code.is_empty() {
        code = if allow_auto { "auto" } else { "en" }.to_string();
    }
    if code == "zh-cn" || code == "zh-tw" {
        code = "zh".to_string();
    }

    let allowed = if allow_auto {
        SUPPORTED_SOURCE_LANGUAGES
    } else {
        SUPPORTED_TARGET_LANGUAGES
    };

    if allowed.contains(&code.as_str()) {
        code
    } else if allow_auto {
        "auto".to_string()
    } else {
        "en".to_string()
    }
}

fn normalized_language_preferences(preferences: &LanguagePreferences) -> LanguagePreferences {
    LanguagePreferences {
        source_language: normalize_language_code(&preferences.source_language, true),
        target_language: normalize_language_code(&preferences.target_language, false),
    }
}

fn should_run_multilingual_transform(text: &str, preferences: &LanguagePreferences) -> bool {
    if text.trim().is_empty() {
        return false;
    }

    let source = preferences.source_language.as_str();
    let target = preferences.target_language.as_str();

    // Default fast path keeps existing behavior for English -> English.
    if source == "en" && target == "en" {
        return false;
    }

    // For multilingual mode, always run transform:
    // - source != en
    // - target != en
    // - source auto (language detection required)
    true
}

async fn transform_multilingual_text(
    transcription_text: &str,
    preferences: &LanguagePreferences,
) -> Result<MultilingualTextResult, String> {
    let base = transcription_text.trim();
    if base.is_empty() {
        return Ok(MultilingualTextResult {
            routing_text: String::new(),
            output_text: String::new(),
            transformed: false,
        });
    }

    if !should_run_multilingual_transform(base, preferences) {
        return Ok(MultilingualTextResult {
            routing_text: base.to_string(),
            output_text: base.to_string(),
            transformed: false,
        });
    }

    Ok(MultilingualTextResult {
        routing_text: base.to_string(),
        output_text: base.to_string(),
        transformed: false,
    })
}

fn normalize_vibe_trigger_phrase(raw: &str) -> String {
    let cleaned = raw.trim().to_lowercase();
    if cleaned.is_empty() {
        return "vibe".to_string();
    }

    cleaned.chars().take(32).collect::<String>()
}

fn normalized_vibe_coding_config(config: &VibeCodingConfig) -> VibeCodingConfig {
    let mut normalized = config.clone();
    normalized.trigger_phrase = normalize_vibe_trigger_phrase(&normalized.trigger_phrase);
    normalized
}

fn is_low_signal_capture(rms: f32, peak: f32, active_ratio: f32) -> bool {
    let weak_metrics = [rms < 0.0045, peak < 0.045, active_ratio < 0.018]
        .into_iter()
        .filter(|weak| *weak)
        .count();
    weak_metrics >= 2
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

fn strip_trigger_phrase_prefix(text: &str, trigger_phrase: &str) -> (String, bool) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (String::new(), false);
    }

    let trigger = normalize_vibe_trigger_phrase(trigger_phrase);
    if trigger.is_empty() {
        return (trimmed.to_string(), false);
    }

    let lowered = trimmed.to_lowercase();
    let candidates = [
        trigger.clone(),
        format!("{}:", trigger),
        format!("{} -", trigger),
        format!("{}.", trigger),
        format!("hey {}", trigger),
        format!("{} mode", trigger),
        format!("{} prompt", trigger),
    ];

    for candidate in candidates {
        let matched = if candidate == trigger {
            lowered == trigger
                || lowered.starts_with(&format!("{} ", trigger))
                || lowered.starts_with(&format!("{}:", trigger))
                || lowered.starts_with(&format!("{}.", trigger))
                || lowered.starts_with(&format!("{} -", trigger))
        } else {
            lowered.starts_with(&candidate)
        };

        if matched {
            let cut_chars = candidate.chars().count();
            let byte_index = trimmed
                .char_indices()
                .nth(cut_chars)
                .map(|(idx, _)| idx)
                .unwrap_or(trimmed.len());
            let stripped = trimmed[byte_index..]
                .trim_start_matches(|c: char| {
                    c == ':' || c == '-' || c == ',' || c == ';' || c.is_whitespace()
                })
                .trim()
                .to_string();
            return (stripped, true);
        }
    }

    (trimmed.to_string(), false)
}

fn is_coding_surface_app(app_name: &str) -> bool {
    let app = app_name.to_lowercase();
    let coding_apps = [
        "cursor",
        "windsurf",
        "visual studio code",
        "vscode",
        "code",
        "visual studio",
        "intellij",
        "pycharm",
        "webstorm",
        "goland",
        "clion",
        "rider",
        "android studio",
        "xcode",
        "zed",
        "sublime",
        "neovim",
        "vim",
        "terminal",
        "powershell",
        "command prompt",
        "cmd",
        "warp",
        "iterm",
        "kitty",
        "alacritty",
        "copilot",
        "chatgpt",
        "claude",
        "aider",
        "replit",
        "bolt",
        "lovable",
    ];

    coding_apps.iter().any(|keyword| app.contains(keyword))
}

fn starts_with_any(text: &str, prefixes: &[&str]) -> bool {
    prefixes
        .iter()
        .any(|prefix| text == *prefix || text.starts_with(&format!("{} ", prefix)))
}

fn coding_prompt_signal_score(text: &str) -> u8 {
    let normalized = normalize_spoken_command_text(text);
    if normalized.is_empty() {
        return 0;
    }

    // Avoid rewriting normal conversation and generic dictation.
    if starts_with_any(
        &normalized,
        &[
            "hello",
            "hi",
            "hey",
            "thanks",
            "thank you",
            "good morning",
            "good night",
            "how are you",
            "what time",
            "what day",
        ],
    ) {
        return 0;
    }

    let mut score = 0_u8;
    let word_count = normalized.split_whitespace().count();

    if word_count >= 6 {
        score += 1;
    }

    if starts_with_any(
        &normalized,
        &[
            "fix",
            "build",
            "write",
            "implement",
            "create",
            "generate",
            "refactor",
            "debug",
            "optimize",
            "add",
            "update",
            "improve",
            "ship",
        ],
    ) {
        score += 2;
    }

    if [
        "function",
        "method",
        "class",
        "component",
        "hook",
        "endpoint",
        "api",
        "schema",
        "migration",
        "typescript",
        "javascript",
        "rust",
        "python",
        "react",
        "next",
        "electron",
        "test",
        "unit test",
        "integration test",
        "stack trace",
        "exception",
        "compile",
        "build error",
        "lint",
        "bug",
        "codebase",
        "repository",
        "pull request",
    ]
    .iter()
    .any(|keyword| normalized.contains(keyword))
    {
        score += 1;
    }

    if [" with ", " using ", " for ", " so that ", " and "]
        .iter()
        .any(|token| normalized.contains(token))
    {
        score += 1;
    }

    score
}

fn should_format_vibe_coding_prompt(
    transcription_text: &str,
    context: &VoiceContext,
    vibe_config: &VibeCodingConfig,
) -> Option<(String, &'static str)> {
    if !vibe_config.enabled {
        return None;
    }

    let trigger_phrase = normalize_vibe_trigger_phrase(&vibe_config.trigger_phrase);
    let (text_without_trigger, stripped_prefix) =
        strip_trigger_phrase_prefix(transcription_text, &trigger_phrase);

    let normalized_text = normalize_spoken_command_text(transcription_text);
    let trigger_with_space = format!("{} ", trigger_phrase);
    let trigger_with_colon = format!("{}:", trigger_phrase);
    let trigger_detected = stripped_prefix
        || normalized_text == trigger_phrase
        || normalized_text.starts_with(&trigger_with_space)
        || normalized_text.starts_with(&trigger_with_colon);

    let source_text = if text_without_trigger.trim().is_empty() {
        if stripped_prefix {
            return None;
        }
        transcription_text.trim().to_string()
    } else {
        text_without_trigger.trim().to_string()
    };
    if source_text.is_empty() {
        return None;
    }

    let coding_app_active = context
        .active_app
        .as_ref()
        .map(|app| is_coding_surface_app(app))
        .unwrap_or(false);
    let coding_signal = coding_prompt_signal_score(&source_text);
    let coding_prompt = coding_signal >= 2;

    let decision = match vibe_config.activation_mode {
        VibeActivationMode::TriggerPhrase => trigger_detected.then_some("trigger_phrase"),
        VibeActivationMode::Automatic => {
            if coding_app_active && coding_signal >= 1 {
                Some("coding_context_app")
            } else if coding_prompt {
                Some("coding_context_prompt")
            } else {
                None
            }
        }
    };

    decision.map(|reason| (source_text, reason))
}

fn split_vibe_prompt_segments(text: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();

    while let Some((index, ch)) = chars.next() {
        let boundary = if ch == '\n' {
            Some(index)
        } else if matches!(ch, '.' | '?' | '!')
            && chars
                .peek()
                .map(|(_, next)| next.is_whitespace())
                .unwrap_or(true)
        {
            Some(index + ch.len_utf8())
        } else {
            None
        };

        if let Some(end) = boundary {
            let segment = text[start..end].trim();
            if !segment.is_empty() {
                segments.push(segment);
            }
            start = if ch == '\n' { index + 1 } else { end };
        }
    }

    let tail = text[start..].trim();
    if !tail.is_empty() {
        segments.push(tail);
    }

    segments
}

fn strip_vibe_leading_filler(segment: &str) -> &str {
    // These forms include explicit pause punctuation from dictation. Keep the
    // list conservative so words that could be identifiers or requirements are
    // never removed merely because they resemble conversational filler.
    const FILLERS: [&str; 8] = [
        "um,",
        "uh,",
        "erm,",
        "hmm,",
        "um...",
        "uh...",
        "you know,",
        "i mean,",
    ];

    let mut remaining = segment.trim_start();
    loop {
        let lower = remaining.to_ascii_lowercase();
        let Some(filler) = FILLERS.iter().find(|filler| lower.starts_with(**filler)) else {
            return remaining;
        };
        remaining = remaining[filler.len()..].trim_start();
    }
}

fn cleaned_vibe_prompt_segments(text: &str) -> Vec<String> {
    let mut cleaned = Vec::<String>::new();
    for segment in split_vibe_prompt_segments(text) {
        let segment = strip_vibe_leading_filler(segment).trim();
        if segment.is_empty() {
            continue;
        }

        // Exact adjacent repeats are a common dictation false start. Matching
        // case-insensitively removes repetition without treating two merely
        // similar requirements as equivalent.
        if cleaned
            .last()
            .is_some_and(|previous| previous.eq_ignore_ascii_case(segment))
        {
            continue;
        }
        cleaned.push(segment.to_string());
    }
    cleaned
}

fn format_vibe_coding_prompt(
    original_text: &str,
    formatting: VibeFormattingStyle,
) -> Result<String, String> {
    let base = original_text.trim();
    if base.is_empty() {
        return Err("Vibe formatting skipped: empty text".to_string());
    }

    let segments = cleaned_vibe_prompt_segments(base);
    if segments.is_empty() {
        return Err("Vibe formatting skipped: no meaningful text".to_string());
    }

    match formatting {
        VibeFormattingStyle::Natural => Ok(segments.join(" ")),
        VibeFormattingStyle::Structured => Ok(segments
            .into_iter()
            .map(|segment| format!("- {segment}"))
            .collect::<Vec<_>>()
            .join("\n")),
    }
}

// ============ Core Voice Commands ============

/// Start listening for voice input
pub async fn start_listening(state: State<'_, AppState>) -> Result<bool, String> {
    state.transcription.ensure_ready()?;

    let mut is_listening = state.is_listening.lock().await;

    if *is_listening {
        // Already listening - just return true instead of error
        return Ok(true);
    }

    // Ensure no stale input stream remains open from a prior interrupted session.
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

    // Start audio streaming
    let preferred_device = {
        let audio = state.audio.lock().await;
        audio.selected_device.clone()
    };

    {
        let streamer = state.streamer.lock().await;
        streamer.start_streaming(preferred_device.as_deref())?;
    }

    *is_listening = true;

    log::info!("Listen OS: Started listening");

    Ok(true)
}

/// Stop capture without transcribing or executing an action.
pub async fn cancel_listening(state: State<'_, AppState>) -> Result<bool, String> {
    {
        let mut is_listening = state.is_listening.lock().await;
        *is_listening = false;
    }

    {
        let streamer = state.streamer.lock().await;
        streamer.stop_streaming();
        streamer.clear_samples();
    }

    {
        let mut is_processing = state.is_processing.lock().await;
        *is_processing = false;
    }

    Ok(true)
}

/// Audio snapshot taken synchronously on the command loop the moment the
/// hold-to-talk shortcut is released. The slow transcription / routing /
/// delivery stages run on a background task so the loop stays responsive to
/// the next shortcut press while processing continues.
#[derive(Debug, Clone)]
pub struct CapturedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// Fast, non-blocking capture teardown for shortcut release: flips the
/// listening flag off, stops the audio stream, raises the processing flag,
/// and snapshots the accumulated samples. This must stay cheap — the native
/// command loop calls it synchronously and spawns the slow
/// [`process_captured_audio`] work afterwards. Previously the loop awaited
/// the full transcription inline, so a multi-second CPU inference blocked
/// every later shortcut event and the overlay looked stuck in Processing.
pub async fn take_capture_for_processing(
    state: State<'_, AppState>,
) -> Result<CapturedAudio, String> {
    // Check if listening
    {
        let is_listening = state.is_listening.lock().await;
        if !*is_listening {
            return Err("Not listening".to_string());
        }
    }

    // Stop listening flag first
    {
        let mut is_listening = state.is_listening.lock().await;
        *is_listening = false;
    }

    // Stop streaming
    {
        let streamer = state.streamer.lock().await;
        streamer.stop_streaming();
    }

    // Set processing state
    {
        let mut is_processing = state.is_processing.lock().await;
        *is_processing = true;
    }

    // Snapshot accumulated audio before any new capture can clear it.
    let (samples, sample_rate) = {
        let streamer = state.streamer.lock().await;
        (
            streamer.get_accumulated_samples(),
            streamer.current_sample_rate(),
        )
    };

    Ok(CapturedAudio {
        samples,
        sample_rate,
    })
}

/// Slow voice-processing stages (silence gating, local transcription,
/// intent routing, execution, delivery). Always clears the processing flag
/// before returning. Runs on a spawned task via the native runtime.
pub async fn process_captured_audio(
    state: State<'_, AppState>,
    captured: CapturedAudio,
    dictation_only: Option<bool>,
) -> Result<VoiceProcessingResult, String> {
    let processing_started = Instant::now();
    let dictation_only = dictation_only.unwrap_or(false);
    let CapturedAudio {
        samples,
        sample_rate,
    } = captured;

    // Calculate audio energy metrics to detect true silence and avoid hallucinated text.
    let rms: f32 = if samples.is_empty() {
        0.0
    } else {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    };
    let peak: f32 = samples
        .iter()
        .map(|s| s.abs())
        .fold(0.0_f32, |acc, value| acc.max(value));
    let active_sample_count = samples.iter().filter(|sample| sample.abs() > 0.012).count();
    let active_ratio = if samples.is_empty() {
        0.0
    } else {
        active_sample_count as f32 / samples.len() as f32
    };
    let duration_ms = (samples.len() as u64 * 1000) / sample_rate as u64;
    log::info!(
        "Audio captured: {} samples, {} ms, RMS: {:.4}, peak: {:.4}, active_ratio: {:.4}",
        samples.len(),
        duration_ms,
        rms,
        peak,
        active_ratio
    );

    let minimum_samples = (sample_rate as usize / 10).max(1);
    if samples.len() < minimum_samples {
        // Less than 100ms
        let mut is_processing = state.is_processing.lock().await;
        *is_processing = false;
        return Err("Recording too short.".to_string());
    }

    // Gate silence before local inference. Besides avoiding wasted CPU this prevents
    // Whisper from hallucinating stock phrases when the microphone captured only noise.
    let is_low_signal = is_low_signal_capture(rms, peak, active_ratio);
    if is_low_signal {
        log::info!(
            "No speech detected before transcription: rms={:.4}, peak={:.4}, active_ratio={:.4}",
            rms,
            peak,
            active_ratio
        );
        let mut is_processing = state.is_processing.lock().await;
        *is_processing = false;
        return Ok(VoiceProcessingResult {
            transcription: TranscriptionResult {
                text: String::new(),
                duration_ms,
                confidence: 0.0,
                is_final: true,
            },
            action: ActionResultResponse {
                action_type: "NoAction".to_string(),
                payload: serde_json::json!({}),
                refined_text: None,
                response_text: None,
                requires_confirmation: false,
                pending_action_id: None,
            },
            executed: true,
            response_text: None,
            session_id: "silent".to_string(),
            delivery_status: state
                .delivery
                .lock()
                .map(|delivery| delivery.snapshot())
                .unwrap_or_default(),
        });
    }

    // Get context
    let context = state.current_context.lock().await.clone();

    // Load dictionary words for recognition hints
    let dictionary_hints = match crate::dictionary::DictionaryStore::new() {
        Ok(store) => store.get_words_for_recognition().unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let (language_preferences, vibe_config) = {
        let config = state.config.lock().await;
        (
            normalized_language_preferences(&config.language_preferences),
            normalized_vibe_coding_config(&config.vibe_coding),
        )
    };
    let transcription_language_hint = language_preferences
        .transcription_language_hint()
        .map(|s| s.to_string());

    let transcription_started = Instant::now();
    let mut transcription = {
        match state
            .transcription
            .transcribe(
                samples,
                sample_rate,
                transcription_language_hint,
                dictionary_hints,
            )
            .await
        {
            Ok(text) => TranscriptionResult {
                text,
                duration_ms,
                confidence: 0.0,
                is_final: true,
            },
            Err(transcription_err) => {
                log::error!("Transcription failed: {}", transcription_err);
                {
                    let mut error_log = state.error_log.lock().await;
                    error_log.log_error_with_details(
                        crate::error_log::ErrorType::Transcription,
                        "Voice transcription failed",
                        transcription_err.clone(),
                    );
                }
                let mut is_processing = state.is_processing.lock().await;
                *is_processing = false;
                return Err(format!("Transcription failed: {}", transcription_err));
            }
        }
    };
    let transcription_ms = transcription_started.elapsed().as_millis();

    let text_lower = transcription.text.trim().to_lowercase();
    // Whisper is known to emit stock phrases such as "thank you" on silence, but
    // those are also perfectly valid things a person can actually say. Only
    // suppress the stock phrase when the captured signal itself was weak. Strong
    // speech must win over the heuristic.
    let is_hallucination =
        should_suppress_stock_hallucination(&text_lower, rms, peak, active_ratio);
    let repetitive_noise = {
        let words: Vec<&str> = text_lower
            .split_whitespace()
            .filter(|w| !w.is_empty())
            .collect();
        if words.len() < 8 {
            false
        } else {
            let unique: std::collections::HashSet<&str> = words.iter().copied().collect();
            unique.len() <= 2
        }
    };

    if transcription.text.trim().is_empty() || is_hallucination || repetitive_noise {
        log::info!(
            "No valid speech detected (RMS: {:.4}, text: '{}', hallucination: {}, repetitive: {})",
            rms,
            transcription.text,
            is_hallucination,
            repetitive_noise
        );
        let mut is_processing = state.is_processing.lock().await;
        *is_processing = false;

        // Return a silent success (NoAction) so frontend just dismisses quietly
        return Ok(VoiceProcessingResult {
            transcription: TranscriptionResult {
                text: String::new(),
                duration_ms,
                confidence: 0.0,
                is_final: true,
            },
            action: ActionResultResponse {
                action_type: "NoAction".to_string(),
                payload: serde_json::json!({}),
                refined_text: None,
                response_text: None,
                requires_confirmation: false,
                pending_action_id: None,
            },
            executed: true,
            response_text: None,
            session_id: "silent".to_string(),
            delivery_status: state
                .delivery
                .lock()
                .map(|delivery| delivery.snapshot())
                .unwrap_or_default(),
        });
    }

    let multilingual =
        match transform_multilingual_text(&transcription.text, &language_preferences).await {
            Ok(result) => result,
            Err(err) => {
                log::warn!("Multilingual transform skipped due to error: {}", err);
                MultilingualTextResult {
                    routing_text: transcription.text.clone(),
                    output_text: transcription.text.clone(),
                    transformed: false,
                }
            }
        };

    let intent_text = if multilingual.routing_text.trim().is_empty() {
        transcription.text.clone()
    } else {
        multilingual.routing_text.clone()
    };
    if !multilingual.output_text.trim().is_empty() {
        transcription.text = multilingual.output_text.clone();
    }

    if multilingual.transformed {
        log::info!(
            "Multilingual transform applied: source={} target={} intent='{}' output='{}'",
            language_preferences.source_language,
            language_preferences.target_language,
            intent_text,
            transcription.text
        );
    }

    // Update conversation history and capture session id.
    let (conv_context, session_id) = {
        let mut conversation = state.conversation.lock().await;

        // Add user message to conversation
        conversation.add_user_message(transcription.text.clone());

        (ConversationContext, conversation.session_id.clone())
    };

    let local_router_action = voice::detect_local_command(&intent_text);

    // Intent routing:
    // 1) Deterministic local router first for explicit command phrases
    // 2) Use local intent parser
    // 3) On failure, default to dictation
    let resolve_intent_action = || async {
        let voice_router = VoiceRouter::new();
        match voice_router
            .process_intent_with_context(&intent_text, &context, &conv_context)
            .await
        {
            Ok(action) => {
                log::info!("Local intent action: {:?}", action.action_type);
                action
            }
            Err(local_err) => {
                log::warn!(
                    "Local intent failed, defaulting to dictation: {}",
                    local_err
                );
                {
                    let mut error_log = state.error_log.lock().await;
                    error_log.log_error_with_details(
                        crate::error_log::ErrorType::LLMProcessing,
                        "AI processing unavailable, using dictation mode",
                        local_err.clone(),
                    );
                }
                ActionResult {
                    action_type: ActionType::TypeText,
                    payload: serde_json::json!({}),
                    refined_text: Some(transcription.text.clone()),
                    response_text: None,
                    requires_confirmation: false,
                }
            }
        }
    };

    let routing_started = Instant::now();
    let mut action = if dictation_only {
        log::info!(
            "Handsfree dictation mode active, bypassing intent routing and forcing TypeText"
        );
        ActionResult {
            action_type: ActionType::TypeText,
            payload: serde_json::json!({
                "dictation_only": true,
                "source": "assistant_handsfree"
            }),
            refined_text: Some(transcription.text.clone()),
            response_text: None,
            requires_confirmation: false,
        }
    } else if let Some(local_action) = local_router_action {
        log::info!(
            "Local router selected action {:?} for transcript '{}'",
            local_action.action_type,
            intent_text
        );
        local_action
    } else if should_route_locally_first(&intent_text, &context) {
        if let Some(local_action) = voice::detect_local_command(&intent_text) {
            log::info!(
                "Local router first selected action {:?} for transcript '{}'",
                local_action.action_type,
                intent_text
            );
            local_action
        } else {
            resolve_intent_action().await
        }
    } else {
        resolve_intent_action().await
    };

    // Deterministic local router fallback.
    // If generic dictation is returned for an obvious command phrase, prefer local action routing.
    if !dictation_only && should_use_local_command_fallback(&intent_text, &context, &action) {
        if let Some(local_action) = voice::detect_local_command(&intent_text) {
            log::info!(
                "Local router fallback selected action {:?} for transcript '{}'",
                local_action.action_type,
                intent_text
            );
            action = local_action;
        }
    }

    if !dictation_only && is_farewell_phrase(&intent_text) && is_power_system_action(&action) {
        log::warn!(
            "Blocked accidental power action {:?} for farewell transcript '{}'",
            action.payload,
            intent_text
        );
        action = ActionResult {
            action_type: ActionType::NoAction,
            payload: serde_json::json!({
                "blocked_action": "power_control",
                "reason": "farewell_phrase"
            }),
            refined_text: None,
            response_text: Some(
                "Ignoring shutdown/restart because this sounded like a goodbye phrase.".to_string(),
            ),
            requires_confirmation: false,
        };
    }

    if multilingual.transformed && action.action_type == ActionType::TypeText {
        action.refined_text = Some(transcription.text.clone());
    }

    if !dictation_only && action.action_type == ActionType::TypeText {
        if let Some((vibe_input, activation_reason)) =
            should_format_vibe_coding_prompt(&transcription.text, &context, &vibe_config)
        {
            // Vibe formatting starts from the raw transcription so generic
            // dictation cleanup cannot discard a meaningful leading word. The
            // explicit trigger phrase is removed from typed output even if
            // formatting itself fails.
            action.refined_text = Some(vibe_input.clone());

            match format_vibe_coding_prompt(&vibe_input, vibe_config.formatting) {
                Ok(formatted_prompt) => {
                    action.refined_text = Some(formatted_prompt);
                    upsert_action_payload_field(
                        &mut action,
                        "vibe_formatted",
                        serde_json::Value::Bool(true),
                    );
                    upsert_action_payload_field(
                        &mut action,
                        "vibe_activation_reason",
                        serde_json::Value::String(activation_reason.to_string()),
                    );
                    upsert_action_payload_field(
                        &mut action,
                        "vibe_formatting",
                        serde_json::Value::String(format!("{:?}", vibe_config.formatting)),
                    );
                }
                Err(err) => {
                    log::warn!("Vibe prompt formatting skipped due to error: {}", err);
                    upsert_action_payload_field(
                        &mut action,
                        "vibe_formatting_error",
                        serde_json::Value::String(err),
                    );
                }
            }
        }
    }
    let routing_ms = routing_started.elapsed().as_millis();

    let confirms_enabled = confirmations_enabled();
    let should_confirm_action =
        action.requires_confirmation || action_requires_confirmation(&action);
    // Safety override: even when global confirmations are disabled, always gate power actions.
    let requires_confirmation = if confirms_enabled {
        should_confirm_action
    } else {
        is_power_system_action(&action)
    };
    let mut pending_action_id: Option<String> = None;

    if !confirms_enabled {
        let mut pending = state.pending_action.lock().await;
        *pending = None;
    }

    if requires_confirmation {
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = chrono::Utc::now().to_rfc3339();
        let summary = summarize_action(&action);

        {
            let mut pending = state.pending_action.lock().await;
            *pending = Some(PendingAction {
                id: id.clone(),
                action: action.clone(),
                transcription: transcription.text.clone(),
                created_at,
            });
        }

        pending_action_id = Some(id);
        if action.response_text.is_none() {
            action.response_text = Some(format!("Confirmation required: {}", summary));
        }
    }

    // Execute immediately only for non-risky actions.
    let execution_started = Instant::now();
    let execute_result = if requires_confirmation {
        Ok(CommandResult {
            success: true,
            message: "Action pending confirmation".to_string(),
            output: None,
        })
    } else {
        execute_action_internal(&action, &state).await
    };
    let execution_ms = execution_started.elapsed().as_millis();

    if !requires_confirmation {
        match &execute_result {
            Ok(result) => {
                if result.success {
                    upsert_action_payload_field(
                        &mut action,
                        "execution_message",
                        serde_json::Value::String(result.message.clone()),
                    );
                    upsert_action_payload_field(
                        &mut action,
                        "executed",
                        serde_json::Value::Bool(true),
                    );

                    if action.response_text.is_none()
                        && !matches!(
                            action.action_type,
                            ActionType::TypeText | ActionType::NoAction
                        )
                    {
                        action.response_text = Some(result.message.clone());
                    }
                } else {
                    upsert_action_payload_field(
                        &mut action,
                        "execution_error",
                        serde_json::Value::String(result.message.clone()),
                    );
                    upsert_action_payload_field(
                        &mut action,
                        "executed",
                        serde_json::Value::Bool(false),
                    );

                    if action.response_text.is_none() {
                        action.response_text =
                            Some(format!("I couldn't complete that: {}", result.message));
                    }
                }
            }
            Err(err) => {
                upsert_action_payload_field(
                    &mut action,
                    "execution_error",
                    serde_json::Value::String(err.clone()),
                );
                upsert_action_payload_field(
                    &mut action,
                    "executed",
                    serde_json::Value::Bool(false),
                );

                if action.response_text.is_none() {
                    action.response_text = Some(format!("I couldn't complete that: {}", err));
                }
            }
        }
    }

    let executed = !requires_confirmation
        && execute_result
            .as_ref()
            .map(|result| result.success)
            .unwrap_or(false);

    // Log execution errors
    if let Err(ref e) = execute_result {
        let mut error_log = state.error_log.lock().await;
        error_log.log_error_with_details(
            crate::error_log::ErrorType::ActionExecution,
            format!("Failed to execute {:?}", action.action_type),
            e.clone(),
        );
    }

    // Track typed text for correction learning
    if executed && action.action_type == ActionType::TypeText {
        if let Some(ref typed) = action.refined_text {
            let mut tracker = state.correction_tracker.lock().await;
            tracker.record_typed(transcription.text.clone(), typed.clone());
        }
    }

    // Update conversation with assistant response
    {
        let mut conversation = state.conversation.lock().await;
        let response_content = if requires_confirmation {
            action
                .response_text
                .clone()
                .unwrap_or_else(|| format!("Pending confirmation: {}", summarize_action(&action)))
        } else {
            action
                .response_text
                .clone()
                .or_else(|| action.refined_text.clone())
                .unwrap_or_else(|| format!("Executed: {:?}", action.action_type))
        };

        conversation.add_assistant_message(
            response_content,
            Some(action.action_type),
            Some(executed),
            Some(action.payload.clone()),
        );

        // Persist conversation to store
        if let Ok(store_guard) = state.conversation_store.lock() {
            if let Some(ref store) = *store_guard {
                let _ = store.save_session(&conversation);
            }
        }
    }

    // Processing logic finished
    let result = VoiceProcessingResult {
        transcription,
        action: ActionResultResponse {
            action_type: format!("{:?}", action.action_type),
            payload: action.payload,
            refined_text: action.refined_text,
            response_text: action.response_text.clone(),
            requires_confirmation,
            pending_action_id: pending_action_id.clone(),
        },
        executed,
        response_text: action.response_text,
        session_id,
        delivery_status: state
            .delivery
            .lock()
            .map(|delivery| delivery.snapshot())
            .unwrap_or_default(),
    };

    // Save to history
    {
        let mut history = state.history.lock().await;
        history.push(result.clone());
        // Keep only last 50
        if history.len() > 50 {
            history.remove(0);
        }
    }

    // Set processing state to false
    {
        let mut is_processing = state.is_processing.lock().await;
        *is_processing = false;
    }

    log::info!(
        "Voice processing timing: capture_ms={}, transcription_ms={}, routing_ms={}, execution_ms={}, total_ms={}, action={:?}, transcript_chars={}",
        duration_ms,
        transcription_ms,
        routing_ms,
        execution_ms,
        processing_started.elapsed().as_millis(),
        action.action_type,
        result.transcription.text.chars().count()
    );
    #[cfg(debug_assertions)]
    eprintln!(
        "[ListenOS timing] processing capture_ms={} transcription_ms={} routing_ms={} execution_ms={} total_ms={} action={:?} transcript_chars={}",
        duration_ms,
        transcription_ms,
        routing_ms,
        execution_ms,
        processing_started.elapsed().as_millis(),
        action.action_type,
        result.transcription.text.chars().count()
    );

    Ok(result)
}

/// Stop listening and process audio.
///
/// Convenience wrapper that performs the fast capture teardown and then the
/// slow processing stages inline. The native runtime prefers
/// [`take_capture_for_processing`] + spawned [`process_captured_audio`] so a
/// long transcription never blocks the shortcut command loop.
pub async fn stop_listening(
    state: State<'_, AppState>,
    dictation_only: Option<bool>,
) -> Result<VoiceProcessingResult, String> {
    let captured = take_capture_for_processing(State::new(&*state)).await?;
    process_captured_audio(state, captured, dictation_only).await
}

/// Get current application status
pub async fn get_status(state: State<'_, AppState>) -> Result<StatusResponse, String> {
    let is_listening = *state.is_listening.lock().await;
    let is_processing = *state.is_processing.lock().await;
    let audio = state.audio.lock().await;
    let streamer = state.streamer.lock().await;
    let audio_status = streamer.snapshot_runtime_status();
    let delivery_status = state
        .delivery
        .lock()
        .map(|delivery| delivery.snapshot())
        .unwrap_or_default();

    Ok(StatusResponse {
        is_listening,
        is_processing,
        is_streaming: streamer.is_streaming(),
        audio_device: audio.selected_device.clone(),
        last_transcription: None,
        audio_status,
        delivery_status,
    })
}

/// Get a pending action waiting for user confirmation.
pub async fn get_pending_action(
    state: State<'_, AppState>,
) -> Result<Option<PendingActionResponse>, String> {
    let pending = state.pending_action.lock().await;
    Ok(pending.as_ref().map(|p| PendingActionResponse {
        id: p.id.clone(),
        action_type: format!("{:?}", p.action.action_type),
        payload: p.action.payload.clone(),
        transcription: p.transcription.clone(),
        summary: summarize_action(&p.action),
        created_at: p.created_at.clone(),
    }))
}

/// Confirm and execute the pending action.
pub async fn confirm_pending_action(state: State<'_, AppState>) -> Result<CommandResult, String> {
    let pending = {
        let pending_guard = state.pending_action.lock().await;
        pending_guard.clone()
    };

    let pending = pending.ok_or_else(|| "No pending action to confirm".to_string())?;
    let execute_result = execute_action_internal(&pending.action, &state).await;

    match execute_result {
        Ok(result) => {
            let mut pending_guard = state.pending_action.lock().await;
            *pending_guard = None;
            Ok(result)
        }
        Err(e) => {
            let mut error_log = state.error_log.lock().await;
            error_log.log_error_with_details(
                crate::error_log::ErrorType::ActionExecution,
                format!(
                    "Failed to execute confirmed {:?}",
                    pending.action.action_type
                ),
                e.clone(),
            );
            Err(e)
        }
    }
}

/// Cancel the pending action without executing it.
pub async fn cancel_pending_action(state: State<'_, AppState>) -> Result<bool, String> {
    let mut pending = state.pending_action.lock().await;
    let had_pending = pending.is_some();
    *pending = None;
    Ok(had_pending)
}

/// Get real-time audio level (0.0 to 1.0) for visualization
pub async fn get_audio_level(state: State<'_, AppState>) -> Result<f32, String> {
    let is_listening = *state.is_listening.lock().await;
    if !is_listening {
        return Ok(0.0);
    }

    let streamer = state.streamer.lock().await;
    Ok(streamer.get_live_level())
}

// ============ Audio Device Commands ============

/// Get list of available audio input devices
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

/// Set the audio input device
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

    let available = crate::audio::AudioState::get_devices()?;
    if !available.iter().any(|device| device.name == cleaned_name) {
        return Err(format!(
            "Audio input device is no longer available: {cleaned_name}"
        ));
    }

    let _update_guard = state.config_update_lock.lock().await;
    let mut next_config = {
        let config = state.config.lock().await;
        config.clone()
    };
    next_config.selected_audio_device = Some(cleaned_name.to_string());
    commit_config(&state, next_config).await?;
    log::info!("Set audio device to {}", cleaned_name);
    Ok(true)
}

// ============ Local Transcription Commands ============

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

fn trim_spoken_punctuation(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches('.')
        .trim_end_matches(',')
        .trim_end_matches('!')
        .trim_end_matches('?')
        .trim()
        .to_string()
}

fn normalize_web_target(target: &str) -> Option<String> {
    let mut normalized = trim_spoken_punctuation(target)
        .replace(" dot ", ".")
        .replace(" slash ", "/")
        .replace(" colon ", ":")
        .replace("  ", " ");
    normalized = normalized.trim().to_string();

    if normalized.is_empty() || normalized.contains(' ') {
        return None;
    }

    let lower = normalized.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(normalized);
    }
    if lower.starts_with("www.") {
        return Some(format!("https://{}", normalized));
    }

    let host = lower.split('/').next().unwrap_or("");
    let host_without_port = host.split(':').next().unwrap_or(host);
    if host_without_port.contains('.') {
        let parts: Vec<&str> = host_without_port.split('.').collect();
        if parts.len() >= 2 {
            let tld = parts.last().copied().unwrap_or("");
            if tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()) {
                return Some(format!("https://{}", normalized));
            }
        }
    }

    None
}

fn is_known_tld(token: &str) -> bool {
    matches!(
        token,
        "com" | "org" | "net" | "io" | "ai" | "dev" | "app" | "co" | "us" | "in" | "edu" | "gov"
    )
}

fn infer_web_target_from_phrase(target: &str, allow_single_word: bool) -> Option<String> {
    let cleaned = trim_spoken_punctuation(target)
        .replace(",", " ")
        .replace("  ", " ");
    let lower = cleaned.to_lowercase();

    if let Some(url) = normalize_web_target(&lower) {
        return Some(url);
    }

    let words: Vec<&str> = lower.split_whitespace().filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return None;
    }

    if words.len() >= 2 {
        let tld = words[words.len() - 1];
        if is_known_tld(tld) {
            let host = words[..words.len() - 1].join("");
            if !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Some(format!("https://{}.{}", host, tld));
            }
        }
    }

    if allow_single_word && words.len() == 1 {
        let token = words[0];
        if token.len() >= 2
            && token.len() <= 48
            && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Some(format!("https://{}.com", token));
        }
    }

    None
}

fn confirmations_enabled() -> bool {
    std::env::var("LISTENOS_REQUIRE_CONFIRMATION")
        .map(|v| {
            let value = v.trim().to_lowercase();
            matches!(value.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

fn normalize_spoken_command_text(text: &str) -> String {
    let mut t = text.trim().to_lowercase();

    loop {
        let mut changed = false;
        for prefix in [
            "please ",
            "can you please ",
            "could you please ",
            "would you please ",
            "can you ",
            "could you ",
            "would you ",
            "hey listenos ",
            "listenos ",
            "assistant ",
            "hey assistant ",
        ] {
            if let Some(rest) = t.strip_prefix(prefix) {
                t = rest.trim().to_string();
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }

    t
}

fn upsert_action_payload_field(action: &mut ActionResult, key: &str, value: serde_json::Value) {
    if let Some(obj) = action.payload.as_object_mut() {
        obj.insert(key.to_string(), value);
    } else {
        let mut payload = serde_json::Map::new();
        payload.insert(key.to_string(), value);
        action.payload = serde_json::Value::Object(payload);
    }
}

fn should_route_locally_first(text: &str, context: &VoiceContext) -> bool {
    if context.mode == VoiceMode::Command {
        return true;
    }

    looks_like_command_phrase(text)
}

fn should_use_local_command_fallback(
    text: &str,
    context: &VoiceContext,
    resolved_action: &ActionResult,
) -> bool {
    if resolved_action.action_type != ActionType::TypeText {
        return false;
    }

    if context.mode == VoiceMode::Command {
        return true;
    }

    looks_like_command_phrase(text)
}

fn looks_like_command_phrase(text: &str) -> bool {
    let t = normalize_spoken_command_text(text);
    if t.is_empty() {
        return false;
    }

    let exact_commands = [
        "mute",
        "unmute",
        "copy",
        "paste",
        "cut",
        "undo",
        "redo",
        "save",
        "refresh",
        "next",
        "previous",
        "play",
        "pause",
        "maximize",
        "minimize",
        "show desktop",
        "task view",
        "desktop view",
        "next desktop",
        "previous desktop",
    ];
    if exact_commands.contains(&t.as_str()) {
        return true;
    }

    if (t.contains("organize") || t.contains("sort") || t.contains("clean up"))
        && t.contains("download")
    {
        return true;
    }

    if t.contains("download")
        && (t.contains("how many")
            || t.contains("how much")
            || t.contains("count")
            || t.contains("number of"))
    {
        return true;
    }

    if t.contains("screenshot") || t.contains("screen shot") || t.contains("capture screen") {
        return true;
    }

    for prefix in ["open ", "launch ", "start ", "visit ", "go to "] {
        if let Some(target) = t.strip_prefix(prefix) {
            if normalize_web_target(target).is_some() {
                return true;
            }
            let target_words = target.split_whitespace().count();
            if (1..=4).contains(&target_words) {
                return true;
            }
            // Long "open ..." phrases are usually dictation, not commands.
            return false;
        }
    }

    let command_prefixes = [
        "search ",
        "search for ",
        "google ",
        "look up ",
        "organize ",
        "sort ",
        "volume ",
        "lock ",
        "take a screenshot",
        "capture screen",
        "screenshot",
        "switch ",
        "desktop view",
        "switch to next desktop",
        "switch to previous desktop",
        "close window",
        "close tab",
        "new tab",
        "new window",
        "run ",
    ];

    command_prefixes.iter().any(|prefix| t.starts_with(prefix))
}

fn is_farewell_phrase(text: &str) -> bool {
    let t = normalize_spoken_command_text(text);
    if t.is_empty() {
        return false;
    }

    let exact = [
        "bye",
        "goodbye",
        "good bye",
        "see you",
        "see ya",
        "talk to you later",
        "catch you later",
        "thanks bye",
        "ok bye",
        "okay bye",
    ];

    exact.contains(&t.as_str())
}

fn is_power_system_action(action: &ActionResult) -> bool {
    if action.action_type != ActionType::SystemControl {
        return false;
    }

    let system_action = action
        .payload
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();

    matches!(system_action.as_str(), "shutdown" | "restart" | "sleep")
}

fn action_requires_confirmation(action: &ActionResult) -> bool {
    match action.action_type {
        ActionType::RunCommand
        | ActionType::SendEmail
        | ActionType::MultiStep
        | ActionType::CustomCommand => true,
        ActionType::SystemControl => {
            let system_action = action
                .payload
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            matches!(
                system_action.as_str(),
                "shutdown"
                    | "restart"
                    | "sleep"
                    | "recycle_bin"
                    | "factory_reset"
                    | "sign_out"
                    | "organize_downloads"
            )
        }
        _ => false,
    }
}

fn summarize_action(action: &ActionResult) -> String {
    match action.action_type {
        ActionType::OpenApp => {
            let app = action
                .payload
                .get("app")
                .and_then(|v| v.as_str())
                .unwrap_or("application");
            format!("Open {}", app)
        }
        ActionType::OpenUrl => {
            let url = action
                .payload
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("URL");
            format!("Open {}", url)
        }
        ActionType::WebSearch => {
            let query = action
                .payload
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("query");
            format!("Search web for \"{}\"", query)
        }
        ActionType::SystemControl => {
            let system_action = action
                .payload
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("system action");
            match system_action {
                "organize_downloads" => "Organize Downloads folder".to_string(),
                "downloads_count" => "Count items in Downloads folder".to_string(),
                "screenshot" => "Take a screenshot".to_string(),
                "open_screenshots_folder" => "Open screenshots folder".to_string(),
                _ => format!("System action: {}", system_action),
            }
        }
        ActionType::RunCommand => {
            let cmd = action
                .payload
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("command");
            format!("Run command: {}", cmd)
        }
        ActionType::SendEmail => "Send email".to_string(),
        ActionType::VolumeControl => {
            let direction = action
                .payload
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or("change");
            format!("Volume {}", direction)
        }
        ActionType::WindowControl => {
            let window_action = action
                .payload
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("window action");
            format!("Window {}", window_action)
        }
        ActionType::KeyboardShortcut => {
            let shortcut = action
                .payload
                .get("shortcut")
                .and_then(|v| v.as_str())
                .unwrap_or("shortcut");
            format!("Keyboard shortcut: {}", shortcut)
        }
        ActionType::TypeText => {
            let text = action.refined_text.as_deref().unwrap_or("text");
            if text.chars().count() > 48 {
                let preview: String = text.chars().take(48).collect();
                format!("Type text: {}...", preview)
            } else {
                format!("Type text: {}", text)
            }
        }
        _ => format!("{:?}", action.action_type),
    }
}

// ============ Action Execution ============

async fn open_url_internal(url: &str) -> Result<CommandResult, String> {
    let normalized_url = normalize_web_target(url)
        .or_else(|| infer_web_target_from_phrase(url, false))
        .unwrap_or_else(|| trim_spoken_punctuation(url));

    if normalized_url.is_empty() {
        return Ok(CommandResult {
            success: false,
            message: "No URL specified".to_string(),
            output: None,
        });
    }

    log::info!("Opening URL: {}", normalized_url);

    #[cfg(windows)]
    {
        use std::process::Command;
        let result = Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", &normalized_url])
            .spawn();

        match result {
            Ok(_) => Ok(CommandResult {
                success: true,
                message: format!("Opened: {}", normalized_url),
                output: None,
            }),
            Err(e) => Err(format!("Failed to open URL: {}", e)),
        }
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let result = Command::new("open").arg(&normalized_url).spawn();

        match result {
            Ok(_) => Ok(CommandResult {
                success: true,
                message: format!("Opened: {}", normalized_url),
                output: None,
            }),
            Err(e) => Err(format!("Failed to open URL: {}", e)),
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let cmd = format!(
            "xdg-open \"{}\" 2>/dev/null || open \"{}\"",
            normalized_url, normalized_url
        );
        run_system_command(cmd).await
    }
}

async fn execute_action_internal(
    action: &ActionResult,
    state: &State<'_, AppState>,
) -> Result<CommandResult, String> {
    match action.action_type {
        // Conversational actions - no system action needed
        ActionType::Respond => Ok(CommandResult {
            success: true,
            message: action
                .response_text
                .clone()
                .unwrap_or_else(|| "Response sent".to_string()),
            output: action.response_text.clone(),
        }),

        ActionType::Clarify => Ok(CommandResult {
            success: true,
            message: action
                .response_text
                .clone()
                .unwrap_or_else(|| "Clarification requested".to_string()),
            output: action.response_text.clone(),
        }),

        // Clipboard actions
        ActionType::ClipboardFormat
        | ActionType::ClipboardTranslate
        | ActionType::ClipboardSummarize
        | ActionType::ClipboardClean => execute_clipboard_action(action, state).await,

        // App integration actions
        ActionType::SpotifyControl => execute_spotify_action(action, state).await,

        ActionType::DiscordControl => execute_discord_action(action, state).await,

        ActionType::SystemControl => execute_system_action(action, state).await,

        ActionType::CustomCommand => execute_custom_command(action, state).await,

        ActionType::TypeText => {
            // Get text from refined_text or payload
            let text = if let Some(ref refined) = action.refined_text {
                refined.clone()
            } else if let Some(payload_text) = action.payload.get("text").and_then(|v| v.as_str()) {
                payload_text.to_string()
            } else {
                String::new()
            };

            if text.is_empty() {
                return Ok(CommandResult {
                    success: false,
                    message: "No text to type".to_string(),
                    output: None,
                });
            }

            type_text_internal(Some(state), text).await
        }

        ActionType::OpenApp => {
            let app = action
                .payload
                .get("app")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_lowercase();

            if app.is_empty() {
                return Ok(CommandResult {
                    success: false,
                    message: "No app specified".to_string(),
                    output: None,
                });
            }

            // If the "app" looks like a URL/domain, open it in browser directly.
            if let Some(url) = normalize_web_target(&app) {
                log::info!(
                    "OpenApp target looked like URL, redirecting to browser: {}",
                    url
                );
                return open_url_internal(&url).await;
            }

            log::info!("Opening app: {}", app);

            #[cfg(windows)]
            {
                use std::process::Command;

                // Try multiple methods in order:
                // 1. Known app mappings (native commands, URI schemes)
                // 2. Start by name
                // 3. URI scheme fallback
                // 4. Web fallback for popular apps

                // Map common app names to Windows commands/URIs
                let known_apps: &[(&str, &str, Option<&str>)] = &[
                    // (name, primary_cmd, web_fallback)
                    // Windows Store Apps - use URI schemes
                    ("settings", "ms-settings:", None),
                    ("windows settings", "ms-settings:", None),
                    ("store", "ms-windows-store:", None),
                    ("microsoft store", "ms-windows-store:", None),
                    ("mail", "outlookmail:", Some("https://outlook.live.com")),
                    ("outlook", "outlookmail:", Some("https://outlook.live.com")),
                    (
                        "calendar",
                        "outlookcal:",
                        Some("https://outlook.live.com/calendar"),
                    ),
                    ("calculator", "calculator:", None),
                    ("camera", "microsoft.windows.camera:", None),
                    ("maps", "bingmaps:", Some("https://maps.google.com")),
                    ("photos", "ms-photos:", None),
                    ("clock", "ms-clock:", None),
                    ("alarms", "ms-clock:", None),
                    ("weather", "bingweather:", Some("https://weather.com")),
                    // Popular apps with URI schemes and web fallbacks
                    ("whatsapp", "whatsapp:", Some("https://web.whatsapp.com")),
                    ("spotify", "spotify:", Some("https://open.spotify.com")),
                    ("discord", "discord:", Some("https://discord.com/app")),
                    ("slack", "slack:", Some("https://app.slack.com")),
                    ("teams", "msteams:", Some("https://teams.microsoft.com")),
                    (
                        "microsoft teams",
                        "msteams:",
                        Some("https://teams.microsoft.com"),
                    ),
                    ("zoom", "zoommtg:", Some("https://zoom.us/join")),
                    ("telegram", "tg:", Some("https://web.telegram.org")),
                    // Browsers
                    ("chrome", "chrome", None),
                    ("google chrome", "chrome", None),
                    ("firefox", "firefox", None),
                    ("edge", "msedge", None),
                    ("microsoft edge", "msedge", None),
                    ("brave", "brave", None),
                    // Common desktop apps
                    ("notepad", "notepad", None),
                    ("word", "winword", None),
                    ("microsoft word", "winword", None),
                    ("excel", "excel", None),
                    ("microsoft excel", "excel", None),
                    ("powerpoint", "powerpnt", None),
                    ("vscode", "code", None),
                    ("visual studio code", "code", None),
                    ("code", "code", None),
                    ("terminal", "wt", None), // Windows Terminal
                    ("cmd", "cmd", None),
                    ("command prompt", "cmd", None),
                    ("powershell", "powershell", None),
                    ("explorer", "explorer", None),
                    ("file explorer", "explorer", None),
                    ("files", "explorer", None),
                    ("task manager", "taskmgr", None),
                    ("control panel", "control", None),
                    // Web-only apps
                    ("youtube", "https://youtube.com", None),
                    ("gmail", "https://gmail.com", None),
                    ("google", "https://google.com", None),
                    ("twitter", "https://x.com", None),
                    ("x", "https://x.com", None),
                    ("facebook", "https://facebook.com", None),
                    ("instagram", "https://instagram.com", None),
                    ("linkedin", "https://linkedin.com", None),
                    ("reddit", "https://reddit.com", None),
                    ("github", "https://github.com", None),
                    ("netflix", "https://netflix.com", None),
                ];

                // Find matching app
                let app_info = known_apps.iter().find(|(name, _, _)| *name == app.as_str());

                if let Some((_, primary_cmd, web_fallback)) = app_info {
                    // Try primary command first
                    let launch_cmd = format!("start {}", primary_cmd);

                    let result = Command::new("cmd").args(["/C", &launch_cmd]).output();

                    match result {
                        Ok(output) if output.status.success() => {
                            return Ok(CommandResult {
                                success: true,
                                message: format!("Opened: {}", app),
                                output: None,
                            });
                        }
                        _ => {
                            // Try web fallback if available
                            if let Some(web_url) = web_fallback {
                                log::info!(
                                    "Primary launch failed, trying web fallback: {}",
                                    web_url
                                );
                                let mut result = open_url_internal(web_url).await?;
                                result.message = format!("Opened {} (web)", app);
                                return Ok(result);
                            }
                        }
                    }
                }

                // Fallback:
                // 1) If executable is available in PATH, start it.
                // 2) Otherwise treat as a likely website (e.g., "notion" -> notion.com).
                let executable_exists = Command::new("cmd")
                    .args(["/C", "where", &app])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);

                if executable_exists {
                    let result = Command::new("cmd").args(["/C", "start", "", &app]).spawn();

                    return match result {
                        Ok(_) => Ok(CommandResult {
                            success: true,
                            message: format!("Opened: {}", app),
                            output: None,
                        }),
                        Err(e) => Err(format!("Failed to open {}: {}", app, e)),
                    };
                }

                if let Some(url) = infer_web_target_from_phrase(&app, true) {
                    log::info!(
                        "App '{}' not found in PATH. Falling back to website open: {}",
                        app,
                        url
                    );
                    return open_url_internal(&url).await;
                }

                Ok(CommandResult {
                    success: false,
                    message: format!("Could not find installed app '{}'", app),
                    output: None,
                })
            }

            #[cfg(target_os = "macos")]
            {
                use std::process::Command;

                // Try open -a first
                let result = Command::new("open").args(["-a", &app]).output();

                if let Ok(output) = result {
                    if output.status.success() {
                        return Ok(CommandResult {
                            success: true,
                            message: format!("Opened: {}", app),
                            output: None,
                        });
                    }
                }

                // Fallback to web version for known apps
                let web_fallback: Option<&str> = match app.as_str() {
                    "whatsapp" => Some("https://web.whatsapp.com"),
                    "spotify" => Some("https://open.spotify.com"),
                    "discord" => Some("https://discord.com/app"),
                    "slack" => Some("https://app.slack.com"),
                    "telegram" => Some("https://web.telegram.org"),
                    _ => None,
                };

                if let Some(url) = web_fallback {
                    let _ = Command::new("open").arg(url).spawn();
                    return Ok(CommandResult {
                        success: true,
                        message: format!("Opened {} (web)", app),
                        output: None,
                    });
                }

                if let Some(url) = infer_web_target_from_phrase(&app, true) {
                    return open_url_internal(&url).await;
                }

                Ok(CommandResult {
                    success: false,
                    message: format!("Could not find installed app '{}'", app),
                    output: None,
                })
            }

            #[cfg(not(any(windows, target_os = "macos")))]
            {
                let cmd = format!("xdg-open {} 2>/dev/null || open {}", app, app);
                run_system_command(cmd).await
            }
        }

        ActionType::WebSearch => {
            let query = action
                .payload
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();

            if query.is_empty() {
                return Ok(CommandResult {
                    success: false,
                    message: "No search query specified".to_string(),
                    output: None,
                });
            }

            log::info!("Searching for: {}", query);

            let encoded_query = query.replace(" ", "+");
            let url = format!("https://www.google.com/search?q={}", encoded_query);

            #[cfg(windows)]
            {
                let mut result = open_url_internal(&url).await?;
                result.message = format!("Searching: {}", query);
                Ok(result)
            }

            #[cfg(not(windows))]
            {
                let cmd = format!("open \"{}\"", url);
                run_system_command(cmd).await
            }
        }

        ActionType::VolumeControl => {
            let direction = action
                .payload
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or("up");

            #[cfg(windows)]
            {
                let key_code = match direction {
                    "up" => 175,
                    "down" => 174,
                    "mute" => 173,
                    _ => 175,
                };
                let cmd = format!(
                    "powershell -Command \"(New-Object -ComObject WScript.Shell).SendKeys([char]{})\"", 
                    key_code
                );
                let _ = run_system_command(cmd).await;
            }

            #[cfg(target_os = "macos")]
            {
                use std::process::Command;
                let script = match direction {
                    "up" => {
                        r#"set volume output volume ((output volume of (get volume settings)) + 10)"#
                    }
                    "down" => {
                        r#"set volume output volume ((output volume of (get volume settings)) - 10)"#
                    }
                    "mute" => {
                        r#"set volume output muted not (output muted of (get volume settings))"#
                    }
                    _ => {
                        r#"set volume output volume ((output volume of (get volume settings)) + 10)"#
                    }
                };
                let _ = Command::new("osascript").args(["-e", script]).output();
            }

            #[cfg(not(any(windows, target_os = "macos")))]
            {
                // Linux: use pactl or amixer
                let cmd = match direction {
                    "up" => "pactl set-sink-volume @DEFAULT_SINK@ +10%",
                    "down" => "pactl set-sink-volume @DEFAULT_SINK@ -10%",
                    "mute" => "pactl set-sink-mute @DEFAULT_SINK@ toggle",
                    _ => "pactl set-sink-volume @DEFAULT_SINK@ +10%",
                };
                let _ = run_system_command(cmd.to_string()).await;
            }

            Ok(CommandResult {
                success: true,
                message: format!("Volume {}", direction),
                output: None,
            })
        }

        ActionType::RunCommand => {
            let cmd = action
                .payload
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if cmd.is_empty() {
                return Err("No command specified".to_string());
            }

            run_system_command(cmd.to_string()).await
        }

        ActionType::OpenUrl => {
            let url = action
                .payload
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            open_url_internal(url).await
        }

        ActionType::SendEmail => {
            // Extract email details
            let to = action
                .payload
                .get("to")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let subject = action
                .payload
                .get("subject")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let body = action
                .payload
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // URL encode the parts
            let encoded_subject = subject.replace(" ", "+");
            let encoded_body = body.replace(" ", "+").replace("\n", "%0A");

            // Open Gmail compose
            let gmail_url = format!(
                "https://mail.google.com/mail/?view=cm&to={}&su={}&body={}",
                to, encoded_subject, encoded_body
            );

            log::info!("Opening email compose: to={}", to);

            #[cfg(windows)]
            {
                let mut result = open_url_internal(&gmail_url).await?;
                result.message = format!("Composing email to: {}", to);
                Ok(result)
            }

            #[cfg(not(windows))]
            {
                let cmd = format!("open \"{}\"", gmail_url);
                run_system_command(cmd).await
            }
        }

        ActionType::MultiStep => {
            // Execute multiple actions in sequence
            let steps = action.payload.get("steps").and_then(|v| v.as_array());

            if let Some(steps) = steps {
                log::info!("Executing {} steps", steps.len());
                let mut success_count = 0usize;
                let mut errors: Vec<String> = Vec::new();

                for (i, step) in steps.iter().enumerate() {
                    let step_action_type = match step["action"].as_str().unwrap_or("") {
                        "open_app" => ActionType::OpenApp,
                        "open_url" => ActionType::OpenUrl,
                        "web_search" => ActionType::WebSearch,
                        "run_command" => ActionType::RunCommand,
                        "type_text" => ActionType::TypeText,
                        "volume_control" => ActionType::VolumeControl,
                        "system_control" => ActionType::SystemControl,
                        "keyboard_shortcut" => ActionType::KeyboardShortcut,
                        "window_control" => ActionType::WindowControl,
                        other => {
                            errors.push(format!("Unknown step action '{}'", other));
                            continue;
                        }
                    };

                    let step_result = ActionResult {
                        action_type: step_action_type,
                        payload: step["payload"].clone(),
                        refined_text: step["refined_text"].as_str().map(|s| s.to_string()),
                        response_text: None,
                        requires_confirmation: false,
                    };

                    log::info!("Step {}: {:?}", i + 1, step_action_type);

                    // Execute and continue regardless of result
                    match Box::pin(execute_action_internal(&step_result, state)).await {
                        Ok(_) => {
                            success_count += 1;
                        }
                        Err(e) => {
                            log::warn!("Step {} failed: {}", i + 1, e);
                            errors.push(format!("Step {} failed: {}", i + 1, e));
                        }
                    }

                    // Small delay between steps
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                }

                if success_count == 0 && !errors.is_empty() {
                    return Err(errors.join("; "));
                }

                let message = if errors.is_empty() {
                    format!("Executed {} steps", steps.len())
                } else {
                    format!(
                        "Executed {}/{} steps. {}",
                        success_count,
                        steps.len(),
                        errors.join("; ")
                    )
                };

                Ok(CommandResult {
                    success: errors.is_empty(),
                    message,
                    output: None,
                })
            } else {
                Ok(CommandResult {
                    success: false,
                    message: "No steps provided".to_string(),
                    output: None,
                })
            }
        }

        ActionType::NoAction => Ok(CommandResult {
            success: true,
            message: "No action required".to_string(),
            output: None,
        }),

        ActionType::KeyboardShortcut => execute_keyboard_shortcut(action).await,

        ActionType::WindowControl => execute_window_control(action).await,
    }
}

fn update_delivery_state(
    state: Option<&State<'_, AppState>>,
    update: impl FnOnce(&mut crate::DeliveryState),
) {
    if let Some(state) = state {
        if let Ok(mut delivery) = state.delivery.lock() {
            update(&mut delivery);
        }
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
            Err(err) => {
                log::warn!("Clipboard set failed on attempt {}: {}", attempt, err);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    Err("Failed to set clipboard for text delivery".to_string())
}

fn perform_delivery_strategy(strategy: DeliveryStrategy, text: &str) -> Result<(), String> {
    use enigo::{Enigo, Key, Keyboard, Settings};

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("Failed to create input injector: {}", e))?;

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
        DeliveryStrategy::ShiftInsert => send_hotkey(&mut enigo, &[Key::Shift], Key::Insert),
        DeliveryStrategy::SimulatedTyping => enigo
            .text(text)
            .map_err(|e| format!("Failed to simulate typing: {}", e)),
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
            .map_err(|e| format!("Failed to press modifier key: {}", e))?;
        std::thread::sleep(std::time::Duration::from_millis(14));
    }

    enigo
        .key(final_key, Direction::Click)
        .map_err(|e| format!("Failed to press delivery key: {}", e))?;
    std::thread::sleep(std::time::Duration::from_millis(18));

    for modifier in modifiers.iter().rev() {
        enigo
            .key(*modifier, Direction::Release)
            .map_err(|e| format!("Failed to release modifier key: {}", e))?;
    }

    Ok(())
}

async fn type_text_internal(
    state: Option<&State<'_, AppState>>,
    text: String,
) -> Result<CommandResult, String> {
    use arboard::Clipboard;

    if text.trim().is_empty() {
        return Err("No text to type".to_string());
    }

    update_delivery_state(state, |delivery| delivery.begin(&text));

    // Let the previous hotkey release/focus transition settle first.
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
            summary: format!("Targeting {}", target_name.clone()),
            recovered_to_clipboard: false,
        });
    });

    let strategies = strategy_chain(surface, &before, &text);
    let mut clipboard = Clipboard::new().ok();
    let previous_clipboard = clipboard
        .as_mut()
        .and_then(|clipboard| clipboard.get_text().ok());

    let mut last_error: Option<String> = None;
    let mut last_result: Option<CommandResult> = None;
    let mut attempted_strategy: Option<DeliveryStrategy> = None;
    let mut attempts_made = 0_u8;

    for (index, strategy) in strategies.iter().enumerate() {
        let attempt = (index + 1) as u8;
        let phase = if attempt > 1 {
            DeliveryPhase::Retrying
        } else {
            DeliveryPhase::Injecting
        };

        update_delivery_state(state, |delivery| {
            delivery.update(DeliveryUpdate {
                phase,
                surface,
                target: target_label.clone(),
                strategy: Some(*strategy),
                attempts: attempt,
                summary: format!("Trying {} for {}", strategy.label(), target_name.clone()),
                recovered_to_clipboard: false,
            });
        });

        if !matches!(strategy, DeliveryStrategy::SimulatedTyping) {
            let Some(clipboard) = clipboard.as_mut() else {
                last_error = Some("Clipboard is unavailable for paste delivery".to_string());
                continue;
            };
            if let Err(err) = set_clipboard_text(clipboard, &text) {
                last_error = Some(err);
                continue;
            }
        }

        // Once an input strategy is actually dispatched, do not automatically
        // try another one. Verification can be inconclusive on browser/editor
        // accessibility surfaces even when the first paste succeeded; retrying
        // in that state is how the same transcription can be inserted twice.
        attempted_strategy = Some(*strategy);
        attempts_made = attempts_made.saturating_add(1);
        if let Err(err) = perform_delivery_strategy(*strategy, &text) {
            last_error = Some(err);
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

        // A successful input dispatch is authoritative. Accessibility/UIA
        // readback can lag or expose only a partial document snapshot even
        // when the target application already accepted the paste. Treat that
        // verification as diagnostic only; converting an inconclusive
        // snapshot into a failure produces a false red error after text has
        // visibly appeared and previously encouraged duplicate retries.
        if let Some(clipboard) = clipboard.as_mut() {
            restore_previous_clipboard(clipboard, previous_clipboard.as_ref());
        }

        let message = if verified {
            format!(
                "Delivered text to {} via {}",
                target_name.clone(),
                strategy.label()
            )
        } else if readback_unavailable {
            format!(
                "Delivered text to {} via {} (readback unavailable)",
                target_name.clone(),
                strategy.label()
            )
        } else {
            format!(
                "Delivered text to {} via {} (readback inconclusive)",
                target_name.clone(),
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

        last_result = Some(CommandResult {
            success: true,
            message,
            output: None,
        });
        break;
    }

    if let Some(result) = last_result {
        return Ok(result);
    }

    let recovered_to_clipboard = clipboard
        .as_mut()
        .map(|clipboard| clipboard.set_text(&text).is_ok())
        .unwrap_or(false);

    let failure_message = if recovered_to_clipboard {
        format!(
            "Failed to deliver text to {}. Transcript was copied to the clipboard for recovery.",
            target_name.clone()
        )
    } else {
        format!(
            "Failed to deliver text to {}. Transcript was kept in the recovery buffer.",
            target_name.clone()
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
            target: target_label.clone(),
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

/// Type text into the active window
pub async fn type_text(state: State<'_, AppState>, text: String) -> Result<CommandResult, String> {
    type_text_internal(Some(&state), text).await
}

/// Run a system command
pub async fn run_system_command(command: String) -> Result<CommandResult, String> {
    use std::process::Command;

    log::info!("Running: {}", command);

    #[cfg(windows)]
    let output = Command::new("cmd")
        .args(["/C", &command])
        .output()
        .map_err(|e| format!("Failed: {}", e))?;

    #[cfg(not(windows))]
    let output = Command::new("sh")
        .args(["-c", &command])
        .output()
        .map_err(|e| format!("Failed: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        Ok(CommandResult {
            success: true,
            message: "Done".to_string(),
            output: Some(stdout),
        })
    } else {
        Err(stderr)
    }
}

// ============ Keyboard Shortcut Helpers ============

/// Get the primary modifier key for the current platform (Cmd on macOS, Ctrl elsewhere)
fn get_primary_modifier() -> enigo::Key {
    #[cfg(target_os = "macos")]
    {
        enigo::Key::Meta
    }
    #[cfg(not(target_os = "macos"))]
    {
        enigo::Key::Control
    }
}

/// Execute a keyboard shortcut (copy, paste, undo, etc.)
async fn execute_keyboard_shortcut(action: &ActionResult) -> Result<CommandResult, String> {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};

    let shortcut = action
        .payload
        .get("shortcut")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if shortcut.is_empty() {
        return Err("No shortcut specified".to_string());
    }

    log::info!("Executing keyboard shortcut: {}", shortcut);

    // Small delay to ensure focus is on the right window
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("Failed to create enigo: {}", e))?;

    // Use Cmd on macOS, Ctrl on Windows/Linux
    let modifier = get_primary_modifier();

    let result = match shortcut {
        "copy" => send_key_combo(&mut enigo, &[modifier], 'c'),
        "paste" => send_key_combo(&mut enigo, &[modifier], 'v'),
        "cut" => send_key_combo(&mut enigo, &[modifier], 'x'),
        "select_all" => send_key_combo(&mut enigo, &[modifier], 'a'),
        "undo" => send_key_combo(&mut enigo, &[modifier], 'z'),
        "redo" => {
            // Cmd+Shift+Z on macOS, Ctrl+Y on Windows
            #[cfg(target_os = "macos")]
            {
                enigo.key(Key::Meta, Direction::Press).ok();
                enigo.key(Key::Shift, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Unicode('z'), Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Shift, Direction::Release).ok();
                enigo.key(Key::Meta, Direction::Release).ok();
                Ok(())
            }
            #[cfg(not(target_os = "macos"))]
            {
                send_key_combo(&mut enigo, &[Key::Control], 'y')
            }
        }
        "save" => send_key_combo(&mut enigo, &[modifier], 's'),
        "find" => send_key_combo(&mut enigo, &[modifier], 'f'),
        "new_tab" => send_key_combo(&mut enigo, &[modifier], 't'),
        "close_tab" => send_key_combo(&mut enigo, &[modifier], 'w'),
        "new_window" => send_key_combo(&mut enigo, &[modifier], 'n'),
        "refresh" => send_key_combo(&mut enigo, &[modifier], 'r'),
        "back" => {
            // Cmd+[ on macOS, Alt+Left on Windows
            #[cfg(target_os = "macos")]
            {
                send_key_combo(&mut enigo, &[Key::Meta], '[')
            }
            #[cfg(not(target_os = "macos"))]
            {
                enigo.key(Key::Alt, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::LeftArrow, Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Alt, Direction::Release).ok();
                Ok(())
            }
        }
        "forward" => {
            // Cmd+] on macOS, Alt+Right on Windows
            #[cfg(target_os = "macos")]
            {
                send_key_combo(&mut enigo, &[Key::Meta], ']')
            }
            #[cfg(not(target_os = "macos"))]
            {
                enigo.key(Key::Alt, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::RightArrow, Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Alt, Direction::Release).ok();
                Ok(())
            }
        }
        _ => Err(format!("Unknown shortcut: {}", shortcut)),
    };

    match result {
        Ok(()) => Ok(CommandResult {
            success: true,
            message: format!("Executed: {}", shortcut),
            output: None,
        }),
        Err(e) => Err(e),
    }
}

/// Helper to send a key combo like Ctrl+C
fn send_key_combo(
    enigo: &mut enigo::Enigo,
    modifiers: &[enigo::Key],
    key: char,
) -> Result<(), String> {
    use enigo::{Direction, Key, Keyboard};

    // Press modifiers
    for modifier in modifiers {
        enigo
            .key(*modifier, Direction::Press)
            .map_err(|e| format!("Failed to press modifier: {}", e))?;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // Press and release the key
    enigo
        .key(Key::Unicode(key), Direction::Click)
        .map_err(|e| format!("Failed to press key: {}", e))?;

    std::thread::sleep(std::time::Duration::from_millis(20));

    // Release modifiers in reverse order
    for modifier in modifiers.iter().rev() {
        enigo
            .key(*modifier, Direction::Release)
            .map_err(|e| format!("Failed to release modifier: {}", e))?;
    }

    Ok(())
}

// ============ Window Control Helpers ============

/// Execute window control commands (minimize, maximize, close, etc.)
async fn execute_window_control(action: &ActionResult) -> Result<CommandResult, String> {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};

    let window_action = action
        .payload
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if window_action.is_empty() {
        return Err("No window action specified".to_string());
    }

    log::info!("Executing window control: {}", window_action);

    // Small delay to ensure focus is on the right window
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("Failed to create enigo: {}", e))?;

    let result = match window_action {
        "minimize" => {
            // Win+Down (minimize)
            #[cfg(windows)]
            {
                enigo.key(Key::Meta, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::DownArrow, Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Meta, Direction::Release).ok();
            }
            #[cfg(target_os = "macos")]
            {
                // Cmd+M
                enigo.key(Key::Meta, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Unicode('m'), Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Meta, Direction::Release).ok();
            }
            Ok(())
        }
        "maximize" => {
            // Win+Up (maximize)
            #[cfg(windows)]
            {
                enigo.key(Key::Meta, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::UpArrow, Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Meta, Direction::Release).ok();
            }
            #[cfg(target_os = "macos")]
            {
                // Ctrl+Cmd+F for fullscreen
                enigo.key(Key::Control, Direction::Press).ok();
                enigo.key(Key::Meta, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Unicode('f'), Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Meta, Direction::Release).ok();
                enigo.key(Key::Control, Direction::Release).ok();
            }
            Ok(())
        }
        "close" => {
            // Alt+F4 (Windows) or Cmd+W (macOS)
            #[cfg(windows)]
            {
                enigo.key(Key::Alt, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::F4, Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Alt, Direction::Release).ok();
            }
            #[cfg(target_os = "macos")]
            {
                enigo.key(Key::Meta, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Unicode('w'), Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Meta, Direction::Release).ok();
            }
            Ok(())
        }
        "switch" => {
            // Alt+Tab
            enigo.key(Key::Alt, Direction::Press).ok();
            std::thread::sleep(std::time::Duration::from_millis(20));
            enigo.key(Key::Tab, Direction::Click).ok();
            std::thread::sleep(std::time::Duration::from_millis(100));
            enigo.key(Key::Alt, Direction::Release).ok();
            Ok(())
        }
        "snap_left" => {
            // Win+Left
            enigo.key(Key::Meta, Direction::Press).ok();
            std::thread::sleep(std::time::Duration::from_millis(20));
            enigo.key(Key::LeftArrow, Direction::Click).ok();
            std::thread::sleep(std::time::Duration::from_millis(20));
            enigo.key(Key::Meta, Direction::Release).ok();
            Ok(())
        }
        "snap_right" => {
            // Win+Right
            enigo.key(Key::Meta, Direction::Press).ok();
            std::thread::sleep(std::time::Duration::from_millis(20));
            enigo.key(Key::RightArrow, Direction::Click).ok();
            std::thread::sleep(std::time::Duration::from_millis(20));
            enigo.key(Key::Meta, Direction::Release).ok();
            Ok(())
        }
        "show_desktop" => {
            // Win+D
            enigo.key(Key::Meta, Direction::Press).ok();
            std::thread::sleep(std::time::Duration::from_millis(20));
            enigo.key(Key::Unicode('d'), Direction::Click).ok();
            std::thread::sleep(std::time::Duration::from_millis(20));
            enigo.key(Key::Meta, Direction::Release).ok();
            Ok(())
        }
        "next_desktop" => {
            #[cfg(windows)]
            {
                // Win+Ctrl+Right
                enigo.key(Key::Meta, Direction::Press).ok();
                enigo.key(Key::Control, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::RightArrow, Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Control, Direction::Release).ok();
                enigo.key(Key::Meta, Direction::Release).ok();
            }
            #[cfg(target_os = "macos")]
            {
                // Ctrl+Right (switch space)
                enigo.key(Key::Control, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::RightArrow, Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Control, Direction::Release).ok();
            }
            Ok(())
        }
        "previous_desktop" => {
            #[cfg(windows)]
            {
                // Win+Ctrl+Left
                enigo.key(Key::Meta, Direction::Press).ok();
                enigo.key(Key::Control, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::LeftArrow, Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Control, Direction::Release).ok();
                enigo.key(Key::Meta, Direction::Release).ok();
            }
            #[cfg(target_os = "macos")]
            {
                // Ctrl+Left (switch space)
                enigo.key(Key::Control, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::LeftArrow, Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Control, Direction::Release).ok();
            }
            Ok(())
        }
        "task_view" => {
            #[cfg(windows)]
            {
                // Win+Tab
                enigo.key(Key::Meta, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Tab, Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Meta, Direction::Release).ok();
            }
            #[cfg(target_os = "macos")]
            {
                // Ctrl+Up (Mission Control)
                enigo.key(Key::Control, Direction::Press).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::UpArrow, Direction::Click).ok();
                std::thread::sleep(std::time::Duration::from_millis(20));
                enigo.key(Key::Control, Direction::Release).ok();
            }
            Ok(())
        }
        "restore" => {
            // Win+Up then Win+Down to restore from minimized/maximized
            enigo.key(Key::Meta, Direction::Press).ok();
            std::thread::sleep(std::time::Duration::from_millis(20));
            enigo.key(Key::UpArrow, Direction::Click).ok();
            std::thread::sleep(std::time::Duration::from_millis(50));
            enigo.key(Key::DownArrow, Direction::Click).ok();
            std::thread::sleep(std::time::Duration::from_millis(20));
            enigo.key(Key::Meta, Direction::Release).ok();
            Ok(())
        }
        _ => Err(format!("Unknown window action: {}", window_action)),
    };

    match result {
        Ok(()) => Ok(CommandResult {
            success: true,
            message: format!("Window: {}", window_action),
            output: None,
        }),
        Err(e) => Err(e),
    }
}

// ============ Clipboard Action Helpers ============

async fn execute_clipboard_action(
    action: &ActionResult,
    state: &State<'_, AppState>,
) -> Result<CommandResult, String> {
    // Get current clipboard content
    let content = {
        let clipboard = state.clipboard.lock().await;
        clipboard.get_current()?
    };

    if content.trim().is_empty() {
        return Ok(CommandResult {
            success: false,
            message: "Clipboard is empty".to_string(),
            output: None,
        });
    }

    let operation = match action.action_type {
        ActionType::ClipboardFormat => "format",
        ActionType::ClipboardTranslate => "translate",
        ActionType::ClipboardSummarize => "summarize",
        ActionType::ClipboardClean => "clean",
        _ => return Err("Invalid clipboard action".to_string()),
    };

    // Process with local clipboard transformer
    let router = VoiceRouter::new();
    let result = router
        .process_clipboard(&content, operation, &action.payload)
        .await?;

    // Set the result back to clipboard
    {
        let clipboard = state.clipboard.lock().await;
        clipboard.set_content(result.clone())?;
    }

    Ok(CommandResult {
        success: true,
        message: format!("Clipboard {}: done", operation),
        output: Some(result),
    })
}

// ============ Integration Action Helpers ============

async fn execute_spotify_action(
    action: &ActionResult,
    state: &State<'_, AppState>,
) -> Result<CommandResult, String> {
    let integrations = state.integrations.lock().await;

    let spotify_action = action
        .payload
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("play_pause");

    let spotify_action = match spotify_action {
        "play" | "pause" | "resume" | "toggle" => "play_pause",
        other => other,
    };

    let spotify_action_id = format!("spotify_{}", spotify_action);

    match integrations.execute("spotify", &spotify_action_id, &action.payload) {
        Ok(result) => Ok(CommandResult {
            success: result.success,
            message: result.message,
            output: result.data.map(|d| d.to_string()),
        }),
        Err(e) => Err(e),
    }
}

async fn execute_discord_action(
    action: &ActionResult,
    state: &State<'_, AppState>,
) -> Result<CommandResult, String> {
    let integrations = state.integrations.lock().await;

    let discord_action = action
        .payload
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("mute");

    let discord_action_id = format!("discord_{}", discord_action);

    match integrations.execute("discord", &discord_action_id, &action.payload) {
        Ok(result) => Ok(CommandResult {
            success: result.success,
            message: result.message,
            output: result.data.map(|d| d.to_string()),
        }),
        Err(e) => Err(e),
    }
}

async fn execute_system_action(
    action: &ActionResult,
    state: &State<'_, AppState>,
) -> Result<CommandResult, String> {
    let integrations = state.integrations.lock().await;

    let system_action = action
        .payload
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("lock");

    let system_action_id = format!("system_{}", system_action);

    match integrations.execute("system", &system_action_id, &action.payload) {
        Ok(result) => Ok(CommandResult {
            success: result.success,
            message: result.message,
            output: result.data.map(|d| d.to_string()),
        }),
        Err(e) => Err(e),
    }
}

// ============ Custom Command Execution ============

async fn execute_custom_command(
    action: &ActionResult,
    state: &State<'_, AppState>,
) -> Result<CommandResult, String> {
    // Get command ID from payload
    let command_id = action.payload.get("command_id").and_then(|v| v.as_str());

    let trigger_phrase = action
        .payload
        .get("trigger_phrase")
        .and_then(|v| v.as_str());

    // Load custom commands store
    let store = custom::CustomCommandsStore::new()?;

    // Find the command either by ID or trigger phrase
    let command = if let Some(id) = command_id {
        store
            .get_all_commands()?
            .into_iter()
            .find(|c| c.id == id && c.enabled)
    } else if let Some(trigger) = trigger_phrase {
        store.find_by_trigger(trigger)?
    } else {
        return Err("No command ID or trigger phrase provided".to_string());
    };

    let command = match command {
        Some(cmd) => cmd,
        None => return Err("Custom command not found or disabled".to_string()),
    };

    log::info!(
        "Executing custom command: {} ({})",
        command.name,
        command.id
    );

    // Execute each action step in sequence
    let mut success_count = 0;
    let total_steps = command.actions.len();

    for (i, step) in command.actions.iter().enumerate() {
        log::info!(
            "Step {}/{}: {} - {:?}",
            i + 1,
            total_steps,
            step.action_type,
            step.payload
        );

        // Apply delay before step (except for first step)
        if step.delay_ms > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(step.delay_ms as u64)).await;
        }

        // Map action_type string to ActionType enum and execute
        let step_action_type = match step.action_type.as_str() {
            "open_app" => ActionType::OpenApp,
            "open_url" => ActionType::OpenUrl,
            "web_search" => ActionType::WebSearch,
            "run_command" => ActionType::RunCommand,
            "type_text" => ActionType::TypeText,
            "volume_control" => ActionType::VolumeControl,
            "spotify_control" => ActionType::SpotifyControl,
            "discord_control" => ActionType::DiscordControl,
            "system_control" => ActionType::SystemControl,
            _ => {
                log::warn!(
                    "Unknown action type in custom command: {}",
                    step.action_type
                );
                continue;
            }
        };

        let step_result = ActionResult {
            action_type: step_action_type,
            payload: step.payload.clone(),
            refined_text: step
                .payload
                .get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            response_text: None,
            requires_confirmation: false,
        };

        // Execute the step (recursively call execute_action_internal)
        match Box::pin(execute_action_internal(&step_result, state)).await {
            Ok(result) => {
                if result.success {
                    success_count += 1;
                }
                log::info!(
                    "Step {}/{} completed: {}",
                    i + 1,
                    total_steps,
                    result.message
                );
            }
            Err(e) => {
                log::warn!("Step {}/{} failed: {}", i + 1, total_steps, e);
            }
        }
    }

    // Record usage
    if let Err(e) = store.record_usage(&command.id) {
        log::warn!("Failed to record command usage: {}", e);
    }

    Ok(CommandResult {
        success: success_count > 0,
        message: format!(
            "Executed '{}': {}/{} steps completed",
            command.name, success_count, total_steps
        ),
        output: None,
    })
}

// ============ Conversation Commands ============

/// Get conversation history
pub async fn get_conversation(
    state: State<'_, AppState>,
) -> Result<Vec<crate::conversation::Message>, String> {
    let conversation = state.conversation.lock().await;
    Ok(conversation.messages.clone())
}

/// Clear conversation history
pub async fn clear_conversation(state: State<'_, AppState>) -> Result<(), String> {
    let mut conversation = state.conversation.lock().await;
    conversation.clear();
    Ok(())
}

/// Start a new conversation session
pub async fn new_conversation_session(state: State<'_, AppState>) -> Result<String, String> {
    let mut conversation = state.conversation.lock().await;

    // Save current session
    if let Ok(store_guard) = state.conversation_store.lock() {
        if let Some(ref store) = *store_guard {
            let _ = store.save_session(&conversation);
        }
    }

    // Create new session
    *conversation = crate::conversation::ConversationMemory::new_session();
    Ok(conversation.session_id.clone())
}

// ============ Clipboard Commands ============

/// Get clipboard content
pub async fn get_clipboard(state: State<'_, AppState>) -> Result<String, String> {
    let clipboard = state.clipboard.lock().await;
    clipboard.get_current()
}

/// Set clipboard content
pub async fn set_clipboard(state: State<'_, AppState>, content: String) -> Result<(), String> {
    let clipboard = state.clipboard.lock().await;
    clipboard.set_content(content)
}

/// Get clipboard history
pub async fn get_clipboard_history(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<crate::clipboard::ClipboardEntry>, String> {
    let clipboard = state.clipboard.lock().await;
    Ok(clipboard.get_history(limit.unwrap_or(20)))
}

// ============ Integration Commands ============

/// Get list of available integrations
pub async fn get_integrations(
    state: State<'_, AppState>,
) -> Result<Vec<crate::integrations::IntegrationInfo>, String> {
    let integrations = state.integrations.lock().await;
    Ok(integrations.list_integrations())
}

/// Enable or disable an integration
pub async fn set_integration_enabled(
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
) -> Result<bool, String> {
    let mut integrations = state.integrations.lock().await;
    Ok(integrations.set_enabled(&name, enabled))
}

// ============ Context Commands ============

/// Set voice mode (dictation or command)
pub async fn set_voice_context(
    state: State<'_, AppState>,
    active_app: Option<String>,
    selected_text: Option<String>,
    mode: String,
) -> Result<bool, String> {
    let mut context = state.current_context.lock().await;

    context.active_app = active_app;
    context.selected_text = selected_text;
    context.mode = match mode.as_str() {
        "command" => VoiceMode::Command,
        _ => VoiceMode::Dictation,
    };
    context.timestamp = chrono::Utc::now().to_rfc3339();

    Ok(true)
}

/// Get current voice context
pub async fn get_voice_context(state: State<'_, AppState>) -> Result<VoiceContext, String> {
    let context = state.current_context.lock().await;
    Ok(context.clone())
}

// ============ Configuration Commands ============

pub async fn get_config(state: State<'_, AppState>) -> Result<crate::config::AppConfig, String> {
    let config = state.config.lock().await;
    Ok(config.clone())
}

/// Normalize a user-facing shortcut string into the canonical spelling shared
/// by native frontends and the global-hotkey adapter.
pub fn normalize_hotkey_string(raw: &str) -> Result<String, String> {
    let cleaned = raw.trim();
    if cleaned.is_empty() {
        return Err("Hotkey cannot be empty".to_string());
    }

    let mut modifiers: Vec<String> = Vec::new();
    let mut key: Option<String> = None;

    for part in cleaned.split('+') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }

        let lower = token.to_lowercase();
        let normalized = match lower.as_str() {
            "ctrl" | "control" => Some("Ctrl".to_string()),
            "alt" | "option" => Some("Alt".to_string()),
            "shift" => Some("Shift".to_string()),
            "win" | "windows" | "meta" | "super" | "cmd" | "command" => Some("Meta".to_string()),
            "spacebar" | "space" => None,
            _ => None,
        };

        if let Some(modifier) = normalized {
            if !modifiers.contains(&modifier) {
                modifiers.push(modifier);
            }
            continue;
        }

        if key.is_none() {
            let normalized_key = if lower == "space" || lower == "spacebar" {
                "Space".to_string()
            } else {
                let mut chars = token.chars();
                match chars.next() {
                    Some(first) => {
                        format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase())
                    }
                    None => continue,
                }
            };
            key = Some(normalized_key);
        }
    }

    let key = key.ok_or_else(|| "Hotkey must include a non-modifier key".to_string())?;
    if modifiers.is_empty() {
        return Err("Hotkey must include at least one modifier key".to_string());
    }

    let mut parts = modifiers;
    parts.push(key);
    Ok(parts.join("+"))
}

/// Validate the two application-wide shortcut bindings without mutating state.
pub fn validate_hotkey_pair(trigger_hotkey: &str, assistant_hotkey: &str) -> Result<(), String> {
    if trigger_hotkey.eq_ignore_ascii_case(assistant_hotkey) {
        return Err("Assistant shortcut must be different from hold-to-talk shortcut".to_string());
    }
    Ok(())
}

async fn commit_config(
    state: &crate::AppState,
    config: crate::config::AppConfig,
) -> Result<bool, String> {
    let mut config = config;
    config.trigger_hotkey = normalize_hotkey_string(&config.trigger_hotkey)?;
    config.assistant_hotkey = normalize_hotkey_string(&config.assistant_hotkey)?;
    config.language_preferences = normalized_language_preferences(&config.language_preferences);
    config.vibe_coding = normalized_vibe_coding_config(&config.vibe_coding);

    let (old_trigger, old_assistant, old_use_gpu) = {
        let current_config = state.config.lock().await;
        (
            current_config.trigger_hotkey.clone(),
            current_config.assistant_hotkey.clone(),
            current_config.use_gpu,
        )
    };
    if old_trigger != config.trigger_hotkey || old_assistant != config.assistant_hotkey {
        log::info!(
            "Updating hotkeys from hold='{}', assistant='{}' to hold='{}', assistant='{}'",
            old_trigger,
            old_assistant,
            config.trigger_hotkey,
            config.assistant_hotkey
        );
        validate_hotkey_pair(&config.trigger_hotkey, &config.assistant_hotkey)?;
    }

    // Persist before replacing the live state so a disk failure cannot leave a
    // value that appears saved for this session but disappears after restart.
    config.save_to_disk()?;
    {
        let mut current_config = state.config.lock().await;
        *current_config = config.clone();
    }
    {
        let mut audio = state.audio.lock().await;
        audio.selected_device = config.selected_audio_device.clone();
    }
    if old_use_gpu != config.use_gpu {
        state.transcription.set_gpu_enabled(config.use_gpu);
    }
    if let Err(err) = config.language_preferences.save_to_disk() {
        log::warn!("Failed to persist language preferences: {}", err);
    }
    if let Err(err) = config.vibe_coding.save_to_disk() {
        log::warn!("Failed to persist vibe coding config: {}", err);
    }
    Ok(true)
}

pub async fn set_config(
    state: State<'_, crate::AppState>,
    config: crate::config::AppConfig,
) -> Result<bool, String> {
    let _update_guard = state.config_update_lock.lock().await;
    commit_config(&state, config).await
}

pub async fn get_trigger_hotkey(state: State<'_, AppState>) -> Result<String, String> {
    let config = state.config.lock().await;
    Ok(config.trigger_hotkey.clone())
}

pub async fn set_trigger_hotkey(
    state: State<'_, AppState>,
    hotkey: String,
) -> Result<String, String> {
    let normalized_trigger = normalize_hotkey_string(&hotkey)?;
    let _update_guard = state.config_update_lock.lock().await;
    let mut next_config = {
        let config = state.config.lock().await;
        config.clone()
    };
    validate_hotkey_pair(&normalized_trigger, &next_config.assistant_hotkey)?;
    next_config.trigger_hotkey = normalized_trigger.clone();
    commit_config(&state, next_config).await?;
    Ok(normalized_trigger)
}

pub async fn get_assistant_hotkey(state: State<'_, AppState>) -> Result<String, String> {
    let config = state.config.lock().await;
    Ok(config.assistant_hotkey.clone())
}

pub async fn set_assistant_hotkey(
    state: State<'_, AppState>,
    hotkey: String,
) -> Result<String, String> {
    let normalized_assistant = normalize_hotkey_string(&hotkey)?;
    let _update_guard = state.config_update_lock.lock().await;
    let mut next_config = {
        let config = state.config.lock().await;
        config.clone()
    };
    validate_hotkey_pair(&next_config.trigger_hotkey, &normalized_assistant)?;
    next_config.assistant_hotkey = normalized_assistant.clone();
    commit_config(&state, next_config).await?;
    Ok(normalized_assistant)
}

pub async fn get_language_preferences(
    state: State<'_, AppState>,
) -> Result<LanguagePreferences, String> {
    let config = state.config.lock().await;
    Ok(normalized_language_preferences(
        &config.language_preferences,
    ))
}

pub async fn set_language_preferences(
    state: State<'_, AppState>,
    source_language: String,
    target_language: String,
) -> Result<LanguagePreferences, String> {
    let normalized = LanguagePreferences {
        source_language: normalize_language_code(&source_language, true),
        target_language: normalize_language_code(&target_language, false),
    };

    let _update_guard = state.config_update_lock.lock().await;
    let mut next_config = {
        let config = state.config.lock().await;
        config.clone()
    };
    next_config.language_preferences = normalized.clone();
    commit_config(&state, next_config).await?;
    Ok(normalized)
}

pub async fn get_vibe_coding_config(
    state: State<'_, AppState>,
) -> Result<VibeCodingConfig, String> {
    let config = state.config.lock().await;
    Ok(normalized_vibe_coding_config(&config.vibe_coding))
}

pub async fn set_vibe_coding_config(
    state: State<'_, AppState>,
    config: VibeCodingConfig,
) -> Result<VibeCodingConfig, String> {
    let normalized = normalized_vibe_coding_config(&config);
    let _update_guard = state.config_update_lock.lock().await;
    let mut next_config = {
        let app_config = state.config.lock().await;
        app_config.clone()
    };
    next_config.vibe_coding = normalized.clone();
    commit_config(&state, next_config).await?;
    Ok(normalized)
}

// ============ Custom Commands ============

/// Get all custom commands
pub async fn get_custom_commands() -> Result<Vec<custom::CustomCommand>, String> {
    let store = custom::CustomCommandsStore::new()?;
    store.get_all_commands()
}

/// Get built-in command templates
pub async fn get_command_templates() -> Result<Vec<custom::CustomCommand>, String> {
    Ok(custom::get_builtin_templates())
}

/// Materialize a built-in template as a fresh enabled custom command.
///
/// Keeping this in the backend avoids making each native frontend reimplement
/// identity/timestamp initialization when onboarding installs templates.
pub async fn install_command_template(
    template_id: String,
) -> Result<custom::CustomCommand, String> {
    let template = custom::get_builtin_templates()
        .into_iter()
        .find(|template| template.id == template_id)
        .ok_or_else(|| format!("Unknown command template: {template_id}"))?;

    let store = custom::CustomCommandsStore::new()?;
    if let Some(existing) = store.get_all_commands()?.into_iter().find(|command| {
        command
            .trigger_phrase
            .eq_ignore_ascii_case(&template.trigger_phrase)
    }) {
        return Ok(existing);
    }

    let mut command = template;
    command.id = uuid::Uuid::new_v4().to_string();
    command.enabled = true;
    command.created_at = chrono::Utc::now();
    command.last_used = None;
    command.use_count = 0;
    store.save_command(&command)?;
    Ok(command)
}

/// Save a custom command
pub async fn save_custom_command(command: custom::CustomCommand) -> Result<(), String> {
    let store = custom::CustomCommandsStore::new()?;
    store.save_command(&command)
}

/// Delete a custom command
pub async fn delete_custom_command(id: String) -> Result<(), String> {
    let store = custom::CustomCommandsStore::new()?;
    store.delete_command(&id)
}

/// Enable or disable a custom command
pub async fn set_custom_command_enabled(id: String, enabled: bool) -> Result<(), String> {
    let store = custom::CustomCommandsStore::new()?;
    store.set_enabled(&id, enabled)
}

/// Export all custom commands to JSON
pub async fn export_custom_commands() -> Result<String, String> {
    let store = custom::CustomCommandsStore::new()?;
    store.export_commands()
}

/// Import custom commands from JSON
pub async fn import_custom_commands(json: String) -> Result<usize, String> {
    let store = custom::CustomCommandsStore::new()?;
    store.import_commands(&json)
}

// ============ History Commands ============

pub async fn get_history(state: State<'_, AppState>) -> Result<Vec<VoiceProcessingResult>, String> {
    Ok(state.history.lock().await.clone())
}

pub async fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    state.history.lock().await.clear();
    Ok(())
}

// ============ Notes Commands ============

pub async fn get_notes(limit: Option<usize>) -> Result<Vec<crate::Note>, String> {
    crate::NotesStore::new()?.get_all_notes(limit)
}

pub async fn create_note(content: String) -> Result<crate::Note, String> {
    crate::NotesStore::new()?.create_note(content)
}

pub async fn update_note(id: String, content: String) -> Result<(), String> {
    crate::NotesStore::new()?.update_note(&id, content)
}

pub async fn delete_note(id: String) -> Result<(), String> {
    crate::NotesStore::new()?.delete_note(&id)
}

pub async fn toggle_note_pin(id: String) -> Result<bool, String> {
    crate::NotesStore::new()?.toggle_pin(&id)
}

/// Transcribe the current captured recording and persist it as a note.
///
/// Kept alongside the other typed backend commands so the native frontend can
/// create voice notes through the same application core as other operations.
pub async fn create_voice_note(state: State<'_, AppState>) -> Result<crate::Note, String> {
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
    if is_low_signal_capture(rms, peak, active_ratio) {
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

// ============ Snippet Commands ============

pub async fn get_snippets() -> Result<Vec<crate::Snippet>, String> {
    crate::SnippetsStore::new()?.get_all_snippets()
}

pub async fn create_snippet(trigger: String, expansion: String) -> Result<crate::Snippet, String> {
    crate::SnippetsStore::new()?.create_snippet(trigger, expansion)
}

pub async fn update_snippet(id: String, trigger: String, expansion: String) -> Result<(), String> {
    crate::SnippetsStore::new()?.update_snippet(&id, trigger, expansion)
}

pub async fn delete_snippet(id: String) -> Result<(), String> {
    crate::SnippetsStore::new()?.delete_snippet(&id)
}

// ============ Dictionary Commands ============

pub async fn get_dictionary_words() -> Result<Vec<crate::DictionaryWord>, String> {
    crate::DictionaryStore::new()?.get_all_words()
}

pub async fn add_dictionary_word(
    word: String,
    is_auto_learned: bool,
) -> Result<crate::DictionaryWord, String> {
    crate::DictionaryStore::new()?.add_word(word, is_auto_learned)
}

pub async fn update_dictionary_word(
    id: String,
    word: String,
    phonetic: Option<String>,
) -> Result<(), String> {
    crate::DictionaryStore::new()?.update_word(&id, word, phonetic)
}

pub async fn delete_dictionary_word(id: String) -> Result<(), String> {
    crate::DictionaryStore::new()?.delete_word(&id)
}

// ============ Error Commands ============

pub async fn get_errors(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<crate::ErrorEntry>, String> {
    Ok(state.error_log.lock().await.get_recent(limit.unwrap_or(20)))
}

pub async fn get_undismissed_errors(
    state: State<'_, AppState>,
) -> Result<Vec<crate::ErrorEntry>, String> {
    Ok(state.error_log.lock().await.get_undismissed())
}

pub async fn dismiss_error(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    Ok(state.error_log.lock().await.dismiss(&id))
}

pub async fn dismiss_all_errors(state: State<'_, AppState>) -> Result<(), String> {
    state.error_log.lock().await.dismiss_all();
    Ok(())
}

// ============ Correction Commands ============

/// Learn dictionary entries inferred from a user's correction.
///
/// Returns the newly learned words. Transport adapters may turn these into
/// notifications, while a native frontend can react to the typed values.
pub async fn learn_correction(
    state: State<'_, AppState>,
    corrected_text: String,
) -> Result<Vec<String>, String> {
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
    Ok(learned)
}

#[cfg(test)]
mod hotkey_tests {
    use super::{
        format_vibe_coding_prompt, is_low_signal_capture, normalize_hotkey_string,
        should_format_vibe_coding_prompt, should_suppress_stock_hallucination,
        validate_hotkey_pair,
    };
    use crate::config::{VibeActivationMode, VibeCodingConfig, VibeFormattingStyle};
    use crate::voice::VoiceContext;

    #[test]
    fn hotkeys_normalize_without_mutating_application_state() {
        assert_eq!(
            normalize_hotkey_string(" control + shift + spacebar ").unwrap(),
            "Ctrl+Shift+Space"
        );
        assert_eq!(normalize_hotkey_string("cmd+K").unwrap(), "Meta+K");
        assert_eq!(
            normalize_hotkey_string("windows + ctrl + space").unwrap(),
            "Meta+Ctrl+Space"
        );
        assert!(normalize_hotkey_string("ctrl + win").is_err());
    }

    #[test]
    fn application_hotkeys_must_remain_distinct() {
        assert!(validate_hotkey_pair("Ctrl+Space", "Ctrl+Alt+Space").is_ok());
        assert!(validate_hotkey_pair("Ctrl+Space", "ctrl+space").is_err());
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
        assert!(!should_suppress_stock_hallucination(
            "hello", 0.001, 0.01, 0.002
        ));
        assert!(should_suppress_stock_hallucination(
            "Thank you.",
            0.006,
            0.12,
            0.01
        ));
        assert!(!should_suppress_stock_hallucination(
            "Thank you!",
            0.009,
            0.14,
            0.08
        ));
    }

    #[test]
    fn silence_gate_tolerates_single_noise_spikes_but_keeps_real_speech() {
        assert!(is_low_signal_capture(0.003, 0.12, 0.006));
        assert!(is_low_signal_capture(0.006, 0.03, 0.008));
        assert!(!is_low_signal_capture(0.007, 0.11, 0.05));
    }

    #[test]
    fn vibe_trigger_phrase_mode_does_not_auto_activate_in_coding_apps() {
        let config = VibeCodingConfig {
            enabled: true,
            activation_mode: VibeActivationMode::TriggerPhrase,
            trigger_phrase: "vibe".to_string(),
            formatting: VibeFormattingStyle::Natural,
        };
        let context = VoiceContext {
            active_app: Some("Cursor".to_string()),
            ..VoiceContext::default()
        };

        assert!(should_format_vibe_coding_prompt(
            "fix the rust compile error in the parser",
            &context,
            &config
        )
        .is_none());

        let activated = should_format_vibe_coding_prompt(
            "vibe: fix the rust compile error in the parser",
            &context,
            &config,
        )
        .expect("trigger phrase should activate formatting");
        assert_eq!(activated.0, "fix the rust compile error in the parser");
        assert_eq!(activated.1, "trigger_phrase");

        let mut disabled = config.clone();
        disabled.enabled = false;
        assert!(should_format_vibe_coding_prompt(
            "vibe: fix the rust compile error in the parser",
            &context,
            &disabled,
        )
        .is_none());
    }

    #[test]
    fn vibe_automatic_mode_ignores_non_coding_dictation() {
        let config = VibeCodingConfig {
            enabled: true,
            activation_mode: VibeActivationMode::Automatic,
            trigger_phrase: "vibe".to_string(),
            formatting: VibeFormattingStyle::Structured,
        };
        let context = VoiceContext {
            active_app: Some("Visual Studio Code".to_string()),
            ..VoiceContext::default()
        };

        assert!(
            should_format_vibe_coding_prompt("thanks for checking this", &context, &config)
                .is_none()
        );
        assert!(should_format_vibe_coding_prompt(
            "fix the rust compile error in the parser",
            &VoiceContext::default(),
            &config,
        )
        .is_some());
    }

    #[test]
    fn vibe_structured_format_preserves_negatives_uncertainty_and_requirements() {
        let input = "Do not delete existing migrations. Maybe reuse the parser if it is safe? Keep --locked exactly as requested.";
        let formatted = format_vibe_coding_prompt(input, VibeFormattingStyle::Structured)
            .expect("structured formatting");

        assert_eq!(
            formatted,
            "- Do not delete existing migrations.\n- Maybe reuse the parser if it is safe?\n- Keep --locked exactly as requested."
        );
        let recovered = formatted
            .lines()
            .map(|line| line.strip_prefix("- ").expect("structured bullet"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(recovered, input);

        for injected in [
            "Constraints:",
            "Acceptance Criteria:",
            "Tests:",
            "Cursor",
            "Claude",
            "ChatGPT",
        ] {
            assert!(
                !formatted.contains(injected),
                "injected content: {injected}"
            );
        }
    }

    #[test]
    fn vibe_natural_format_preserves_meaning_while_removing_clear_spoken_noise() {
        let input = "  Maybe keep this as-is, and do not add tests I did not request.  ";
        let formatted = format_vibe_coding_prompt(input, VibeFormattingStyle::Natural)
            .expect("natural formatting");
        assert_eq!(
            formatted,
            "Maybe keep this as-is, and do not add tests I did not request."
        );

        let dictated = "Um, do not change src/lib.rs. Do not change src/lib.rs. Uh, maybe use gpui-base if it already fits?";
        let structured = format_vibe_coding_prompt(dictated, VibeFormattingStyle::Structured)
            .expect("structured spoken cleanup");
        assert_eq!(
            structured,
            "- do not change src/lib.rs.\n- maybe use gpui-base if it already fits?"
        );

        for meaningful_leading_word in [
            "LikeButton should keep its existing API.",
            "So should remain the variable name.",
            "WellKnownType must not be renamed.",
        ] {
            assert_eq!(
                format_vibe_coding_prompt(meaningful_leading_word, VibeFormattingStyle::Natural)
                    .expect("meaningful leading word"),
                meaningful_leading_word
            );
        }
    }
}
