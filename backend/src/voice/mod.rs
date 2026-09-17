//! Local voice intent routing for ListenOS.
//!
//! Speech-to-text is owned by `crate::transcription`; this module only turns
//! already-transcribed text into deterministic local actions.

use serde::{Deserialize, Serialize};

/// Extract a number from text (for brightness level, volume, etc.)
fn extract_number(text: &str) -> Option<u32> {
    text.split_whitespace()
        .find_map(|word| word.parse::<u32>().ok())
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

    // Domain-like: x.com, docs.google.com/path, etc.
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

fn infer_web_target_from_phrase(target: &str) -> Option<String> {
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

    // Support spoken domains like "x com", "open ai com", "docs example io"
    if words.len() >= 2 {
        let tld = words[words.len() - 1];
        if is_known_tld(tld) {
            let host = words[..words.len() - 1].join("");
            if !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Some(format!("https://{}.{}", host, tld));
            }
        }
    }

    None
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

/// Post-process dictation text to clean up common issues
fn post_process_dictation(text: &str) -> String {
    let mut result = text.to_string();

    // Remove multiple consecutive spaces
    while result.contains("  ") {
        result = result.replace("  ", " ");
    }

    // Trim leading/trailing whitespace
    result = result.trim().to_string();

    // Fix spacing around punctuation
    result = result.replace(" .", ".");
    result = result.replace(" ,", ",");
    result = result.replace(" ?", "?");
    result = result.replace(" !", "!");
    result = result.replace(" :", ":");
    result = result.replace(" ;", ";");

    // Fix common spoken punctuation that wasn't converted
    result = result.replace(" period", ".");
    result = result.replace(" comma", ",");
    result = result.replace(" question mark", "?");
    result = result.replace(" exclamation point", "!");
    result = result.replace(" exclamation mark", "!");
    result = result.replace(" colon", ":");
    result = result.replace(" semicolon", ";");
    result = result.replace(" new line", "\n");
    result = result.replace(" new paragraph", "\n\n");

    // Only remove fillers if they appear at the start of sentences
    // to avoid removing valid uses in the middle
    let filler_starters = ["Um ", "Uh ", "Like ", "So ", "Well "];
    for filler in filler_starters {
        if result.starts_with(filler) {
            result = result[filler.len()..].to_string();
            // Re-capitalize the first letter
            if let Some(first_char) = result.chars().next() {
                result = first_char.to_uppercase().to_string() + &result[first_char.len_utf8()..];
            }
        }
    }

    // Ensure first letter is capitalized (if it starts with a letter)
    if let Some(first_char) = result.chars().next() {
        if first_char.is_alphabetic() && first_char.is_lowercase() {
            result = first_char.to_uppercase().to_string() + &result[first_char.len_utf8()..];
        }
    }

    organize_long_instruction_dictation(&result)
}

fn split_dictation_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();

    while let Some((index, ch)) = chars.next() {
        if matches!(ch, '.' | '?' | '!')
            && chars
                .peek()
                .map(|(_, next)| next.is_whitespace())
                .unwrap_or(true)
        {
            let end = index + ch.len_utf8();
            let sentence = text[start..end].trim();
            if !sentence.is_empty() {
                sentences.push(sentence.to_string());
            }
            start = end;
        }
    }

    let tail = text[start..].trim();
    if !tail.is_empty() {
        sentences.push(tail.to_string());
    }
    sentences
}

fn normalized_sentence_key(sentence: &str) -> String {
    sentence
        .trim()
        .trim_end_matches(['.', '!', '?'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn organize_long_instruction_dictation(text: &str) -> String {
    if text.contains('\n') {
        return text.to_string();
    }

    let sentences = split_dictation_sentences(text);
    if sentences.len() < 2 {
        return text.to_string();
    }

    let mut deduplicated = Vec::with_capacity(sentences.len());
    let mut previous_key: Option<String> = None;
    for sentence in sentences {
        let key = normalized_sentence_key(&sentence);
        if previous_key.as_deref() == Some(key.as_str()) {
            continue;
        }
        previous_key = Some(key);
        deduplicated.push(sentence);
    }

    let plain = deduplicated.join(" ");
    let word_count = plain.split_whitespace().count();
    if deduplicated.len() < 3 || word_count < 20 {
        return plain;
    }

    let lowered = plain.to_lowercase();
    let instruction_markers = [
        "make sure",
        "should",
        "need to",
        "i need",
        "i want",
        "please",
        "fix",
        "remove",
        "add",
        "change",
        "keep",
        "do not",
        "don't",
        "also",
        "same for",
        "error",
        "issue",
        "problem",
    ];
    let marker_count = instruction_markers
        .iter()
        .filter(|marker| lowered.contains(**marker))
        .count();

    if marker_count < 2 {
        return plain;
    }

    deduplicated
        .into_iter()
        .map(|sentence| format!("- {sentence}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Context metadata associated with a voice request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceContext {
    pub active_app: Option<String>,
    pub selected_text: Option<String>,
    pub os: String,
    pub timestamp: String,
    pub mode: VoiceMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceMode {
    Dictation, // Just transcribe and type
    Command,   // Parse as command and execute
}

impl Default for VoiceContext {
    fn default() -> Self {
        Self {
            active_app: None,
            selected_text: None,
            os: std::env::consts::OS.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            mode: VoiceMode::Dictation,
        }
    }
}

/// Routed action result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub action_type: ActionType,
    pub payload: serde_json::Value,
    pub refined_text: Option<String>,
    /// AI response text for conversational actions (Respond, Clarify)
    pub response_text: Option<String>,
    /// Whether this action requires user confirmation
    pub requires_confirmation: bool,
}

impl ActionResult {
    /// Create a simple action result
    pub fn action(action_type: ActionType, payload: serde_json::Value) -> Self {
        Self {
            action_type,
            payload,
            refined_text: None,
            response_text: None,
            requires_confirmation: false,
        }
    }

    /// Create a type text action
    pub fn type_text(text: String) -> Self {
        Self {
            action_type: ActionType::TypeText,
            payload: serde_json::json!({}),
            refined_text: Some(text),
            response_text: None,
            requires_confirmation: false,
        }
    }

    /// Create a conversational response
    pub fn respond(text: String) -> Self {
        Self {
            action_type: ActionType::Respond,
            payload: serde_json::json!({}),
            refined_text: None,
            response_text: Some(text),
            requires_confirmation: false,
        }
    }

    /// Create a clarification request
    pub fn clarify(question: String) -> Self {
        Self {
            action_type: ActionType::Clarify,
            payload: serde_json::json!({}),
            refined_text: None,
            response_text: Some(question),
            requires_confirmation: false,
        }
    }
}

/// Conversation context placeholder for future multi-turn routing.
#[derive(Debug, Clone, Default)]
pub struct ConversationContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionType {
    // Core actions
    TypeText,
    RunCommand,
    OpenApp,
    OpenUrl,
    WebSearch,
    VolumeControl,
    SendEmail,
    MultiStep,
    NoAction,

    // Conversational actions
    Respond, // AI responds conversationally (e.g., answering a question)
    Clarify, // AI asks for clarification

    // Clipboard actions
    ClipboardFormat,    // Format clipboard content
    ClipboardTranslate, // Translate clipboard content
    ClipboardSummarize, // Summarize clipboard content
    ClipboardClean,     // Clean up clipboard text

    // App integration actions
    SpotifyControl, // Control Spotify (play, pause, next, etc.)
    DiscordControl, // Control Discord (mute, deafen, etc.)
    SystemControl,  // System controls (brightness, lock, etc.)

    // Custom commands
    CustomCommand, // Execute a user-defined custom command

    // Keyboard shortcuts (copy, paste, undo, etc.)
    KeyboardShortcut, // Execute a keyboard shortcut

    // Window management
    WindowControl, // Control windows (minimize, maximize, close, etc.)
}

/// Deterministic local router for already-transcribed speech.
pub struct VoiceRouter;

impl VoiceRouter {
    pub fn new() -> Self {
        Self
    }

    /// Process text with full conversation context for multi-turn dialogues
    pub async fn process_intent_with_context(
        &self,
        text: &str,
        _voice_context: &VoiceContext,
        _conv_context: &ConversationContext,
    ) -> Result<ActionResult, String> {
        // 1. Deterministic local command execution
        if let Some(action) = self.detect_local_command(text) {
            log::info!("Local command detected: {:?}", action.action_type);
            return Ok(action);
        }

        // 2. No remote LLM fallback: default to local dictation
        Ok(ActionResult::type_text(post_process_dictation(text)))
    }

    /// Detect if the text is a simple command that can be handled locally
    ///
    /// IMPORTANT: This should only catch UNAMBIGUOUS commands.
    /// When in doubt, let the LLM decide (it can distinguish dictation from commands).
    /// Only detect commands that are:
    /// 1. Short (1-4 words typically)
    /// 2. Start with a clear command verb
    /// 3. Have no ambiguity with normal dictation
    fn detect_local_command(&self, text: &str) -> Option<ActionResult> {
        // Pre-process: clean up transcription artifacts
        let t = normalize_spoken_command_text(text);
        // Remove trailing punctuation
        let t = t
            .trim_end_matches('.')
            .trim_end_matches(',')
            .trim_end_matches('!')
            .trim_end_matches('?');
        // Remove leading punctuation that Whisper sometimes adds
        let t = t.trim_start_matches(',').trim_start_matches('.').trim();
        // Normalize multiple spaces and remove commas between words (Whisper artifact)
        let t = t.replace(", ", " ").replace("  ", " ");

        // Count words - if extremely long, likely dictation/paragraph
        let word_count = t.split_whitespace().count();
        if word_count > 24 {
            return None; // Too long to be a simple command
        }

        let has_negation = t.contains("don't")
            || t.contains("dont")
            || t.contains("do not")
            || t.contains("not ")
            || t.contains("never")
            || t.contains("cancel");

        // System controls (strict matching for high-risk power actions)
        let explicit_shutdown = t == "shutdown"
            || t == "shut down"
            || t.starts_with("shutdown ")
            || t.starts_with("shut down ")
            || t.starts_with("power off")
            || t.starts_with("turn off computer")
            || t.starts_with("turn off pc");
        if explicit_shutdown && !has_negation {
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "shutdown"}),
            ));
        }

        let explicit_restart = t == "restart"
            || t == "reboot"
            || t.starts_with("restart ")
            || t.starts_with("reboot ");
        if explicit_restart && !has_negation {
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "restart"}),
            ));
        }
        if t.contains("lock")
            && (t.contains("computer")
                || t.contains("screen")
                || t.contains("pc")
                || t.contains("my")
                || t == "lock")
        {
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "lock"}),
            ));
        }
        let explicit_sleep = t == "sleep"
            || t.starts_with("sleep ")
            || t.starts_with("put computer to sleep")
            || t.starts_with("put pc to sleep");
        if explicit_sleep && !has_negation {
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "sleep"}),
            ));
        }
        let mentions_downloads = t.contains("download") || t.contains("downloads folder");
        let wants_download_count = mentions_downloads
            && (t.contains("how many")
                || t.contains("how much")
                || t.contains("count")
                || t.contains("number of")
                || t.contains("how many files"));
        let wants_organize_downloads = mentions_downloads
            && (t.contains("organize") || t.contains("sort") || t.contains("clean up"));
        let wants_screenshot =
            t.contains("screenshot") || t.contains("screen shot") || t.contains("capture screen");
        let wants_open = t.contains("open") || t.contains("show");
        let wants_screenshot_folder = t.contains("screenshot folder")
            || t.contains("screenshots folder")
            || (t.contains("screenshot") && t.contains("folder"))
            || (t.contains("screen shot") && t.contains("folder"));

        if wants_screenshot && wants_open && t.contains("folder") {
            return Some(ActionResult::action(
                ActionType::MultiStep,
                serde_json::json!({
                    "steps": [
                        { "action": "system_control", "payload": { "action": "screenshot" } },
                        { "action": "system_control", "payload": { "action": "open_screenshots_folder" } }
                    ]
                }),
            ));
        }

        if wants_open && wants_screenshot_folder {
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "open_screenshots_folder"}),
            ));
        }

        if wants_download_count && wants_organize_downloads && wants_screenshot {
            return Some(ActionResult::action(
                ActionType::MultiStep,
                serde_json::json!({
                    "steps": [
                        { "action": "system_control", "payload": { "action": "downloads_count" } },
                        { "action": "system_control", "payload": { "action": "organize_downloads" } },
                        { "action": "system_control", "payload": { "action": "screenshot" } }
                    ]
                }),
            ));
        }
        if wants_download_count && wants_organize_downloads {
            return Some(ActionResult::action(
                ActionType::MultiStep,
                serde_json::json!({
                    "steps": [
                        { "action": "system_control", "payload": { "action": "downloads_count" } },
                        { "action": "system_control", "payload": { "action": "organize_downloads" } }
                    ]
                }),
            ));
        }
        if wants_organize_downloads && wants_screenshot {
            return Some(ActionResult::action(
                ActionType::MultiStep,
                serde_json::json!({
                    "steps": [
                        { "action": "system_control", "payload": { "action": "organize_downloads" } },
                        { "action": "system_control", "payload": { "action": "screenshot" } }
                    ]
                }),
            ));
        }
        if wants_download_count && wants_screenshot {
            return Some(ActionResult::action(
                ActionType::MultiStep,
                serde_json::json!({
                    "steps": [
                        { "action": "system_control", "payload": { "action": "downloads_count" } },
                        { "action": "system_control", "payload": { "action": "screenshot" } }
                    ]
                }),
            ));
        }
        if wants_download_count {
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "downloads_count"}),
            ));
        }
        if wants_organize_downloads {
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "organize_downloads"}),
            ));
        }
        if wants_screenshot {
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "screenshot"}),
            ));
        }
        // Bluetooth control - distinguish between toggle and settings
        if t.contains("bluetooth") {
            // Check if user wants to turn on/off/enable/disable
            if t.contains("turn on") || t.contains("enable") || t.contains("switch on") {
                return Some(ActionResult::action(
                    ActionType::SystemControl,
                    serde_json::json!({"action": "bluetooth_toggle", "enable": true}),
                ));
            } else if t.contains("turn off") || t.contains("disable") || t.contains("switch off") {
                return Some(ActionResult::action(
                    ActionType::SystemControl,
                    serde_json::json!({"action": "bluetooth_toggle", "enable": false}),
                ));
            } else if t.contains("toggle") {
                return Some(ActionResult::action(
                    ActionType::SystemControl,
                    serde_json::json!({"action": "bluetooth_toggle"}),
                ));
            }
            // Default: open settings
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "bluetooth"}),
            ));
        }
        // WiFi control - distinguish between toggle and settings
        if t.contains("wifi") || t.contains("wi-fi") {
            // Check if user wants to turn on/off/enable/disable
            if t.contains("turn on") || t.contains("enable") || t.contains("switch on") {
                return Some(ActionResult::action(
                    ActionType::SystemControl,
                    serde_json::json!({"action": "wifi_toggle", "enable": true}),
                ));
            } else if t.contains("turn off") || t.contains("disable") || t.contains("switch off") {
                return Some(ActionResult::action(
                    ActionType::SystemControl,
                    serde_json::json!({"action": "wifi_toggle", "enable": false}),
                ));
            } else if t.contains("toggle") {
                return Some(ActionResult::action(
                    ActionType::SystemControl,
                    serde_json::json!({"action": "wifi_toggle"}),
                ));
            }
            // Default: open settings
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "wifi"}),
            ));
        }
        if t.contains("brightness") {
            // Try to extract level
            let level = extract_number(&t).unwrap_or(50);
            if t.contains("up") || t.contains("increase") {
                return Some(ActionResult::action(
                    ActionType::SystemControl,
                    serde_json::json!({"action": "brightness", "level": "up"}),
                ));
            } else if t.contains("down") || t.contains("decrease") || t.contains("dim") {
                return Some(ActionResult::action(
                    ActionType::SystemControl,
                    serde_json::json!({"action": "brightness", "level": "down"}),
                ));
            }
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "brightness", "level": level}),
            ));
        }
        if t.contains("night light") || t.contains("night mode") || t.contains("blue light") {
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "night_light"}),
            ));
        }
        if t.contains("do not disturb") || t.contains("dnd") || t.contains("focus mode") {
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "dnd"}),
            ));
        }
        if t.contains("empty")
            && (t.contains("trash") || t.contains("recycle") || t.contains("bin"))
        {
            return Some(ActionResult::action(
                ActionType::SystemControl,
                serde_json::json!({"action": "recycle_bin"}),
            ));
        }

        // Volume control
        if t.contains("volume") || t.contains("louder") || t.contains("quieter") {
            let direction = if t.contains("up") || t.contains("louder") || t.contains("increase") {
                "up"
            } else if t.contains("down")
                || t.contains("quieter")
                || t.contains("decrease")
                || t.contains("lower")
            {
                "down"
            } else if t.contains("mute") {
                "mute"
            } else {
                "up" // default
            };
            return Some(ActionResult::action(
                ActionType::VolumeControl,
                serde_json::json!({"direction": direction}),
            ));
        }
        if t == "mute" || t == "unmute" {
            return Some(ActionResult::action(
                ActionType::VolumeControl,
                serde_json::json!({"direction": "mute"}),
            ));
        }

        // App/URL opening - check websites first, then apps
        // Match patterns: "open chrome", "open x.com", "go to x.com", "visit github.com"
        let open_patterns = [
            "open ", "open, ", "open. ", "launch ", "start ", "go to ", "visit ",
        ];
        let has_open_prefix = open_patterns.iter().any(|p| t.starts_with(p));

        // Also match if it's just "open X" without space issues
        let words: Vec<&str> = t.split_whitespace().collect();
        let is_open_command = words.len() >= 2
            && (words[0] == "open"
                || words[0] == "launch"
                || words[0] == "start"
                || words[0] == "visit"
                || (words[0] == "go" && words.get(1) == Some(&"to"))
                || words[0] == "open,"
                || words[0] == "open.");

        if has_open_prefix || is_open_command {
            // Clean up target - remove command words and punctuation
            let raw_target = if is_open_command {
                if words[0] == "go" && words.get(1) == Some(&"to") {
                    words[2..].join(" ")
                } else {
                    words[1..].join(" ")
                }
            } else {
                t.replace("open, ", "")
                    .replace("open. ", "")
                    .replace("open ", "")
                    .replace("launch ", "")
                    .replace("start ", "")
                    .replace("go to ", "")
                    .replace("visit ", "")
            };

            let app_name = trim_spoken_punctuation(&raw_target);
            let app_words: Vec<&str> = app_name.split_whitespace().collect();
            let has_spoken_tld = app_words.last().map(|w| is_known_tld(w)).unwrap_or(false);
            let prefers_web_target = t.starts_with("visit ")
                || t.starts_with("go to ")
                || app_name.contains('.')
                || app_name.starts_with("www.")
                || app_name.starts_with("http://")
                || app_name.starts_with("https://")
                || raw_target.contains(" dot ")
                || has_spoken_tld;

            if app_name.is_empty() {
                return None;
            }

            // Direct URL/domain opening (e.g., "open x.com", "visit github.com")
            if prefers_web_target {
                if let Some(url) = infer_web_target_from_phrase(&app_name) {
                    return Some(ActionResult::action(
                        ActionType::OpenUrl,
                        serde_json::json!({"url": url}),
                    ));
                }
            }

            // Long natural-language phrases that start with "open ..." are often dictation.
            // Let LLM decide instead of forcing app launch.
            if app_name.split_whitespace().count() > 4 {
                return None;
            }

            log::info!("Detected app open command: '{}' -> app: '{}'", t, app_name);

            // OS-aware system app aliases
            // These map user-friendly names to the correct app name for the execute_action handler
            let system_aliases: &[(&str, &str)] = match std::env::consts::OS {
                "windows" => &[
                    ("file explorer", "explorer"),
                    ("files", "explorer"),
                    ("explorer", "explorer"),
                    ("my computer", "explorer"),
                    ("this pc", "explorer"),
                    ("finder", "explorer"), // macOS user on Windows
                    ("settings", "settings"),
                    ("control panel", "control panel"),
                    ("task manager", "task manager"),
                    ("terminal", "terminal"),
                    ("command prompt", "cmd"),
                    ("cmd", "cmd"),
                    ("powershell", "powershell"),
                    ("notepad", "notepad"),
                    ("calculator", "calculator"),
                    ("calendar", "calendar"),
                    ("camera", "camera"),
                    ("clock", "clock"),
                    ("photos", "photos"),
                    ("store", "store"),
                    ("microsoft store", "store"),
                ],
                "macos" => &[
                    ("finder", "finder"),
                    ("files", "finder"),
                    ("file explorer", "finder"),
                    ("explorer", "finder"),
                    ("settings", "system preferences"),
                    ("system preferences", "system preferences"),
                    ("terminal", "terminal"),
                    ("activity monitor", "activity monitor"),
                    ("task manager", "activity monitor"),
                ],
                "linux" => &[
                    ("files", "nautilus"),
                    ("file explorer", "nautilus"),
                    ("file manager", "nautilus"),
                    ("settings", "gnome-control-center"),
                    ("terminal", "gnome-terminal"),
                ],
                _ => &[],
            };

            // Check system aliases first
            for (alias, app) in system_aliases {
                if app_name == *alias || app_name.contains(alias) {
                    return Some(ActionResult::action(
                        ActionType::OpenApp,
                        serde_json::json!({"app": *app}),
                    ));
                }
            }

            // Check if it's a website that should open in browser
            // Use EXACT match to avoid "netflix" matching "x"
            let web_apps = [
                ("youtube", "https://youtube.com"),
                ("gmail", "https://gmail.com"),
                ("twitter", "https://twitter.com"),
                ("x", "https://x.com"), // Must be exact match
                ("facebook", "https://facebook.com"),
                ("instagram", "https://instagram.com"),
                ("linkedin", "https://linkedin.com"),
                ("reddit", "https://reddit.com"),
                ("github", "https://github.com"),
                ("netflix", "https://netflix.com"),
                ("amazon", "https://amazon.com"),
            ];

            for (name, url) in web_apps {
                // Use exact match for short names like "x", contains for others
                let matches = if name.len() <= 2 {
                    app_name == *name
                } else {
                    app_name == *name || app_name.contains(name)
                };
                if matches {
                    return Some(ActionResult::action(
                        ActionType::OpenUrl,
                        serde_json::json!({"url": url}),
                    ));
                }
            }

            // Otherwise treat as app (execute_action has more mappings)
            return Some(ActionResult::action(
                ActionType::OpenApp,
                serde_json::json!({"app": app_name}),
            ));
        }

        // Web search
        if t.starts_with("search ")
            || t.starts_with("google ")
            || t.starts_with("search for ")
            || t.starts_with("look up ")
        {
            let query = t
                .replace("search for ", "")
                .replace("search ", "")
                .replace("google ", "")
                .replace("look up ", "")
                .trim()
                .to_string();
            if !query.is_empty() {
                return Some(ActionResult::action(
                    ActionType::WebSearch,
                    serde_json::json!({"query": query}),
                ));
            }
        }

        // Media control (Spotify/general)
        // Simple controls: play, pause, next, previous
        if t == "play"
            || t == "pause"
            || t == "stop music"
            || t == "resume"
            || t == "resume music"
            || t == "play music"
            || t == "pause music"
        {
            return Some(ActionResult::action(
                ActionType::SpotifyControl,
                serde_json::json!({"action": "play_pause"}),
            ));
        }
        if t == "next" || t == "skip" || t == "next song" || t == "next track" {
            return Some(ActionResult::action(
                ActionType::SpotifyControl,
                serde_json::json!({"action": "next"}),
            ));
        }
        if t == "previous" || t == "previous song" || t == "last song" {
            return Some(ActionResult::action(
                ActionType::SpotifyControl,
                serde_json::json!({"action": "previous"}),
            ));
        }

        // Play specific song/artist - "play [song name]" or "play [artist]"
        // Opens Spotify, searches, and plays the first result
        if t.starts_with("play ") && (2..=6).contains(&word_count) {
            let song_query = t.replace("play ", "").trim().to_string();
            if !song_query.is_empty() && song_query != "music" {
                return Some(ActionResult::action(
                    ActionType::SpotifyControl,
                    serde_json::json!({
                        "action": "play_song",
                        "query": song_query
                    }),
                ));
            }
        }

        // Keyboard shortcuts (clipboard, editing)
        if t == "copy" || t == "copy that" || t == "copy this" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "copy"}),
            ));
        }
        if t == "paste" || t == "paste that" || t == "paste it" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "paste"}),
            ));
        }
        if t == "cut" || t == "cut that" || t == "cut this" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "cut"}),
            ));
        }
        if t == "select all" || t == "select everything" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "select_all"}),
            ));
        }
        if t == "undo" || t == "undo that" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "undo"}),
            ));
        }
        if t == "redo" || t == "redo that" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "redo"}),
            ));
        }
        if t == "save" || t == "save file" || t == "save this" || t == "save it" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "save"}),
            ));
        }
        if t == "find" || t == "search here" || t == "find in page" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "find"}),
            ));
        }
        if t == "new tab" || t == "open new tab" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "new_tab"}),
            ));
        }
        if t == "close tab" || t == "close this tab" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "close_tab"}),
            ));
        }
        if t == "new window" || t == "open new window" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "new_window"}),
            ));
        }
        if t == "refresh" || t == "reload" || t == "reload page" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "refresh"}),
            ));
        }
        if t == "go back" || t == "back" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "back"}),
            ));
        }
        if t == "go forward" || t == "forward" {
            return Some(ActionResult::action(
                ActionType::KeyboardShortcut,
                serde_json::json!({"shortcut": "forward"}),
            ));
        }

        // Window management
        if t == "minimize" || t == "minimize window" || t == "minimize this" {
            return Some(ActionResult::action(
                ActionType::WindowControl,
                serde_json::json!({"action": "minimize"}),
            ));
        }
        if t == "maximize"
            || t == "maximize window"
            || t == "maximize this"
            || t == "full screen"
            || t == "fullscreen"
        {
            return Some(ActionResult::action(
                ActionType::WindowControl,
                serde_json::json!({"action": "maximize"}),
            ));
        }
        if t == "close window" || t == "close this window" || t == "close this" {
            return Some(ActionResult::action(
                ActionType::WindowControl,
                serde_json::json!({"action": "close"}),
            ));
        }
        if t == "switch window" || t == "switch app" || t == "next window" || t == "alt tab" {
            return Some(ActionResult::action(
                ActionType::WindowControl,
                serde_json::json!({"action": "switch"}),
            ));
        }
        if t == "snap left" || t == "move left" || t == "window left" {
            return Some(ActionResult::action(
                ActionType::WindowControl,
                serde_json::json!({"action": "snap_left"}),
            ));
        }
        if t == "snap right" || t == "move right" || t == "window right" {
            return Some(ActionResult::action(
                ActionType::WindowControl,
                serde_json::json!({"action": "snap_right"}),
            ));
        }
        if t == "show desktop" || t == "desktop" || t == "minimize all" {
            return Some(ActionResult::action(
                ActionType::WindowControl,
                serde_json::json!({"action": "show_desktop"}),
            ));
        }
        if t == "next desktop"
            || t == "switch to next desktop"
            || t == "desktop right"
            || (t.contains("switch") && t.contains("desktop") && t.contains("next"))
        {
            return Some(ActionResult::action(
                ActionType::WindowControl,
                serde_json::json!({"action": "next_desktop"}),
            ));
        }
        if t == "previous desktop"
            || t == "switch to previous desktop"
            || t == "desktop left"
            || (t.contains("switch")
                && t.contains("desktop")
                && (t.contains("previous") || t.contains("back")))
        {
            return Some(ActionResult::action(
                ActionType::WindowControl,
                serde_json::json!({"action": "previous_desktop"}),
            ));
        }
        if t == "task view"
            || t == "show desktops"
            || t == "mission control"
            || t == "switch desktop view"
            || t == "desktop view"
        {
            return Some(ActionResult::action(
                ActionType::WindowControl,
                serde_json::json!({"action": "task_view"}),
            ));
        }
        if t == "restore" || t == "restore window" {
            return Some(ActionResult::action(
                ActionType::WindowControl,
                serde_json::json!({"action": "restore"}),
            ));
        }

        // Quick responses (time, date, etc.)
        if t.contains("what time") || t.contains("what's the time") || t == "time" {
            let now = chrono::Local::now();
            let time_str = now.format("%I:%M %p").to_string();
            let tz = now.format("%Z").to_string();
            return Some(ActionResult::respond(format!("It's {} ({})", time_str, tz)));
        }
        if t.contains("what day")
            || t.contains("what's today")
            || t.contains("today's date")
            || t.contains("what date")
        {
            let now = chrono::Local::now();
            let date_str = now.format("%A, %B %d, %Y").to_string();
            return Some(ActionResult::respond(format!("Today is {}", date_str)));
        }

        None
    }

    /// Process clipboard operations with LLM
    pub async fn process_clipboard(
        &self,
        content: &str,
        operation: &str,
        params: &serde_json::Value,
    ) -> Result<String, String> {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Ok(String::new());
        }

        let output = match operation {
            "format" => {
                let format_type = params
                    .get("format")
                    .and_then(|v| v.as_str())
                    .unwrap_or("paragraph")
                    .to_lowercase();

                let normalized = post_process_dictation(trimmed);
                if format_type.contains("bullet") || format_type.contains("list") {
                    normalized
                        .split(['.', '\n'])
                        .map(|line| line.trim())
                        .filter(|line| !line.is_empty())
                        .map(|line| format!("- {}", line))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    normalized
                }
            }
            "translate" => return Err(
                "Clipboard translation requires an LLM provider; currently disabled in local mode"
                    .to_string(),
            ),
            "summarize" => {
                let normalized = post_process_dictation(trimmed);
                let sentences: Vec<&str> = normalized
                    .split(['.', '!', '?'])
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                if sentences.is_empty() {
                    normalized
                } else {
                    let take = sentences
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(". ");
                    if take.ends_with('.') {
                        take
                    } else {
                        format!("{}.", take)
                    }
                }
            }
            "clean" => post_process_dictation(trimmed),
            _ => return Err(format!("Unknown clipboard operation: {}", operation)),
        };

        Ok(output)
    }
}

impl Default for VoiceRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// Public helper for deterministic command routing.
/// Returns `Some(ActionResult)` only for unambiguous command phrases.
pub fn detect_local_command(text: &str) -> Option<ActionResult> {
    VoiceRouter::new().detect_local_command(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_instruction_dictation_is_deduplicated_and_structured() {
        let input = "One issue is the error animation lasts too long. One issue is the error animation lasts too long. Please make sure it dismisses quickly. Also add a real error icon. The output should stay organized for long prompts.";
        let processed = post_process_dictation(input);

        assert_eq!(
            processed,
            "- One issue is the error animation lasts too long.\n- Please make sure it dismisses quickly.\n- Also add a real error icon.\n- The output should stay organized for long prompts."
        );
    }

    #[test]
    fn ordinary_short_dictation_stays_natural() {
        assert_eq!(
            post_process_dictation("hello there. how are you?"),
            "Hello there. how are you?"
        );
    }

    #[test]
    fn detects_specific_song_play_requests() {
        let action = detect_local_command("play 505 by Arctic Monkeys")
            .expect("song playback should be routed locally");

        assert_eq!(action.action_type, ActionType::SpotifyControl);
        assert_eq!(action.payload["action"], "play_song");
        assert_eq!(action.payload["query"], "505 by arctic monkeys");
    }

    #[test]
    fn detects_youtube_music_play_requests() {
        let action = detect_local_command("play some lofi music on YouTube")
            .expect("youtube music playback should be routed locally");

        assert_eq!(action.action_type, ActionType::SpotifyControl);
        assert_eq!(action.payload["action"], "play_song");
        assert_eq!(action.payload["query"], "some lofi music on youtube");
    }
}
