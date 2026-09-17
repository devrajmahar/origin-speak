export type UnlistenFn = () => void;

function desktopBridge(): NonNullable<Window["listenOS"]> {
  if (typeof window === "undefined" || !window.listenOS) {
    throw new Error("ListenOS desktop bridge is unavailable");
  }
  return window.listenOS;
}

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return desktopBridge().invoke<T>(command, args);
}

export async function listenDesktopEvent<T>(
  eventName: string,
  callback: (event: { payload: T }) => void,
): Promise<UnlistenFn> {
  return desktopBridge().on<T>(eventName, (payload) => callback({ payload }));
}

// ============ Types ============

export interface TranscriptionResult {
  text: string;
  duration_ms: number;
  confidence: number;
  is_final: boolean;
}

export interface ActionResult {
  action_type: string;
  payload: Record<string, unknown>;
  refined_text: string | null;
  response_text: string | null;
  requires_confirmation: boolean;
  pending_action_id: string | null;
}

export interface PendingAction {
  id: string;
  action_type: string;
  payload: Record<string, unknown>;
  transcription: string;
  summary: string;
  created_at: string;
}

export interface VoiceProcessingResult {
  transcription: TranscriptionResult;
  action: ActionResult;
  executed: boolean;
  response_text: string | null;
  session_id: string;
  delivery_status: DeliveryStatusSnapshot;
}

export type AudioHealthPhase = "Idle" | "Starting" | "Healthy" | "Recovering" | "Error";
export type DeliveryPhase =
  | "Idle"
  | "Preparing"
  | "Injecting"
  | "Verifying"
  | "Retrying"
  | "Succeeded"
  | "RecoverableFailure";
export type TargetSurfaceKind = "Terminal" | "Browser" | "CodeEditor" | "TextField" | "Unknown";

export interface AudioRuntimeStatus {
  phase: AudioHealthPhase;
  device_name: string | null;
  restart_count: number;
  last_error: string | null;
  last_callback_age_ms: number | null;
}

export interface DeliveryStatusSnapshot {
  phase: DeliveryPhase;
  target: string | null;
  surface: TargetSurfaceKind;
  strategy: string | null;
  attempts: number;
  summary: string;
  transcript_preview: string | null;
  recovered_to_clipboard: boolean;
  updated_at: string;
}

export interface ConversationMessage {
  id: string;
  role: "User" | "Assistant" | "System";
  content: string;
  timestamp: string;
  action_taken: string | null;
  action_success: boolean | null;
}

export interface ClipboardEntry {
  id: string;
  content: string;
  content_type: string;
  timestamp: string;
  source_app: string | null;
  word_count: number;
  char_count: number;
}

export interface IntegrationInfo {
  name: string;
  description: string;
  available: boolean;
  enabled: boolean;
  actions: IntegrationAction[];
}

export interface IntegrationAction {
  id: string;
  name: string;
  description: string;
  parameters: ActionParameter[];
  example_phrases: string[];
}

export interface ActionParameter {
  name: string;
  param_type: string;
  required: boolean;
  description: string;
}

export interface CustomCommand {
  id: string;
  name: string;
  trigger_phrase: string;
  description: string;
  actions: ActionStep[];
  enabled: boolean;
  created_at: string;
  last_used: string | null;
  use_count: number;
}

export interface ActionStep {
  id: string;
  action_type: string;
  payload: Record<string, unknown>;
  delay_ms: number;
  description: string | null;
}

export interface StatusResponse {
  is_listening: boolean;
  is_processing: boolean;
  is_streaming: boolean;
  audio_device: string | null;
  last_transcription: string | null;
  audio_status: AudioRuntimeStatus;
  delivery_status: DeliveryStatusSnapshot;
}

// ============ Voice Commands ============

// Start listening - begins native audio recording
export async function startListening(): Promise<boolean> {
  return invoke("start_listening");
}

// Cancel recording without transcription or action execution.
export async function cancelListening(): Promise<boolean> {
  return invoke("cancel_listening");
}

// Stop listening - processes audio with the configured local transcription model.
export async function stopListening(dictationOnly = false): Promise<VoiceProcessingResult> {
  return invoke("stop_listening", {
    dictationOnly,
    dictation_only: dictationOnly,
  });
}

// Get current status
export async function getStatus(): Promise<StatusResponse> {
  return invoke("get_status");
}

// Get real-time audio level (0.0 to 1.0) for visualization
export async function getAudioLevel(): Promise<number> {
  return invoke("get_audio_level");
}

// ============ Config Commands ============

export async function getTriggerHotkey(): Promise<string> {
  return invoke("get_trigger_hotkey");
}

export async function setTriggerHotkey(hotkey: string): Promise<string> {
  return invoke("set_trigger_hotkey", { hotkey });
}

export async function getAssistantHotkey(): Promise<string> {
  return invoke("get_assistant_hotkey");
}

export async function setAssistantHotkey(hotkey: string): Promise<string> {
  return invoke("set_assistant_hotkey", { hotkey });
}

export interface LanguagePreferences {
  source_language: string;
  target_language: string;
}

export type VibeActivationMode = "ManualOnly" | "SmartAuto" | "Always";
export type VibeTargetTool = "Generic" | "Cursor" | "Windsurf" | "Claude" | "ChatGPT" | "Copilot";
export type VibeDetailLevel = "Concise" | "Balanced" | "Detailed";

export interface VibeCodingConfig {
  enabled: boolean;
  activation_mode: VibeActivationMode;
  trigger_phrase: string;
  target_tool: VibeTargetTool;
  detail_level: VibeDetailLevel;
  include_constraints: boolean;
  include_acceptance_criteria: boolean;
  include_test_notes: boolean;
  concise_output: boolean;
}

export async function getLanguagePreferences(): Promise<LanguagePreferences> {
  return invoke("get_language_preferences");
}

export async function setLanguagePreferences(
  sourceLanguage: string,
  targetLanguage: string,
): Promise<LanguagePreferences> {
  return invoke("set_language_preferences", {
    // Keep both spellings while older native builds are still in circulation.
    sourceLanguage,
    targetLanguage,
    // Keep snake_case aliases for compatibility across bridge behavior.
    source_language: sourceLanguage,
    target_language: targetLanguage,
  });
}

export async function getVibeCodingConfig(): Promise<VibeCodingConfig> {
  return invoke("get_vibe_coding_config");
}

export async function setVibeCodingConfig(
  config: VibeCodingConfig,
): Promise<VibeCodingConfig> {
  return invoke("set_vibe_coding_config", { config });
}

export interface LocalModelInfo {
  id: string;
  label: string;
  filename: string;
  path: string;
  downloaded: boolean;
  selected: boolean;
}

export interface TranscriptionSettings {
  model: string;
}

export type TranscriptionRuntimePhase =
  | "Ready"
  | "ModelMissing"
  | "Downloading"
  | "Transcribing"
  | "Error";

export interface TranscriptionRuntimeStatus {
  model: string;
  model_path: string;
  model_downloaded: boolean;
  phase: TranscriptionRuntimePhase;
  last_error: string | null;
}

export async function listLocalModels(): Promise<LocalModelInfo[]> {
  return invoke("list_local_models");
}

export async function getTranscriptionSettings(): Promise<TranscriptionSettings> {
  return invoke("get_transcription_settings");
}

export async function setTranscriptionModel(model: string): Promise<TranscriptionSettings> {
  return invoke("set_transcription_model", { model });
}

export async function getTranscriptionRuntimeStatus(): Promise<TranscriptionRuntimeStatus> {
  return invoke("get_transcription_runtime_status");
}

export async function downloadLocalModel(model: string): Promise<LocalModelInfo> {
  return invoke("download_local_model", { model });
}

// ============ Action Commands ============

export async function typeText(text: string): Promise<{ success: boolean; message: string }> {
  return invoke("type_text", { text });
}

export async function runSystemCommand(command: string): Promise<{ success: boolean; message: string; output?: string }> {
  return invoke("run_system_command", { command });
}

export async function getPendingAction(): Promise<PendingAction | null> {
  return invoke("get_pending_action");
}

export async function confirmPendingAction(): Promise<{ success: boolean; message: string; output?: string }> {
  return invoke("confirm_pending_action");
}

export async function cancelPendingAction(): Promise<boolean> {
  return invoke("cancel_pending_action");
}

// ============ Audio Device Commands ============

export interface AudioDevice {
  name: string;
  is_default: boolean;
}

export async function getAudioDevices(): Promise<AudioDevice[]> {
  return invoke("get_audio_devices");
}

export async function setAudioDevice(deviceName: string): Promise<boolean> {
  return invoke("set_audio_device", { deviceName });
}

// ============ History Commands ============

export async function getHistory(): Promise<VoiceProcessingResult[]> {
  return invoke("get_history");
}

export async function clearHistory(): Promise<void> {
  return invoke("clear_history");
}

// ============ Window Commands ============

export async function hideAssistant(): Promise<void> {
  return invoke("hide_assistant");
}

export async function showDashboard(): Promise<void> {
  return invoke("show_dashboard");
}

// ============ Event Listeners ============

export function onShortcutPressed(callback: () => void): Promise<UnlistenFn> {
  return listenDesktopEvent("shortcut-pressed", () => {
    callback();
  });
}

export function onShortcutReleased(callback: () => void): Promise<UnlistenFn> {
  return listenDesktopEvent("shortcut-released", () => {
    callback();
  });
}

export function onAssistantShortcut(callback: () => void): Promise<UnlistenFn> {
  return listenDesktopEvent("assistant-shortcut", () => {
    callback();
  });
}

// ============ Conversation Commands ============

export async function getConversation(): Promise<ConversationMessage[]> {
  return invoke("get_conversation");
}

export async function clearConversation(): Promise<void> {
  return invoke("clear_conversation");
}

export async function newConversationSession(): Promise<string> {
  return invoke("new_conversation_session");
}

// ============ Clipboard Commands ============

export async function getClipboard(): Promise<string> {
  return invoke("get_clipboard");
}

export async function setClipboard(content: string): Promise<void> {
  return invoke("set_clipboard", { content });
}

export async function getClipboardHistory(limit?: number): Promise<ClipboardEntry[]> {
  return invoke("get_clipboard_history", { limit });
}

// ============ Integration Commands ============

export async function getIntegrations(): Promise<IntegrationInfo[]> {
  return invoke("get_integrations");
}

export async function setIntegrationEnabled(name: string, enabled: boolean): Promise<boolean> {
  return invoke("set_integration_enabled", { name, enabled });
}

// ============ Custom Commands ============

export async function getCustomCommands(): Promise<CustomCommand[]> {
  return invoke("get_custom_commands");
}

export async function getCommandTemplates(): Promise<CustomCommand[]> {
  return invoke("get_command_templates");
}

export async function saveCustomCommand(command: CustomCommand): Promise<void> {
  return invoke("save_custom_command", { command });
}

export async function deleteCustomCommand(id: string): Promise<void> {
  return invoke("delete_custom_command", { id });
}

export async function setCustomCommandEnabled(id: string, enabled: boolean): Promise<void> {
  return invoke("set_custom_command_enabled", { id, enabled });
}

export async function exportCustomCommands(): Promise<string> {
  return invoke("export_custom_commands");
}

export async function importCustomCommands(json: string): Promise<number> {
  return invoke("import_custom_commands", { json });
}

// ============ Autostart Commands ============

export async function getAutostartEnabled(): Promise<boolean> {
  return invoke("get_autostart_enabled");
}

export async function setAutostartEnabled(enabled: boolean): Promise<boolean> {
  return invoke("set_autostart_enabled", { enabled });
}

// ============ Notes Types & Commands ============

export interface Note {
  id: string;
  content: string;
  timestamp: string;
  tags: string[];
  is_pinned: boolean;
}

export async function getNotes(limit?: number): Promise<Note[]> {
  return invoke("get_notes", { limit });
}

export async function createNote(content: string): Promise<Note> {
  return invoke("create_note", { content });
}

export async function updateNote(id: string, content: string): Promise<void> {
  return invoke("update_note", { id, content });
}

export async function deleteNote(id: string): Promise<void> {
  return invoke("delete_note", { id });
}

export async function toggleNotePin(id: string): Promise<boolean> {
  return invoke("toggle_note_pin", { id });
}

export async function createVoiceNote(): Promise<Note> {
  return invoke("create_voice_note");
}

// ============ Snippets Types & Commands ============

export interface Snippet {
  id: string;
  trigger: string;
  expansion: string;
  category: string;
  created_at: string;
  last_used: string | null;
  use_count: number;
}

export async function getSnippets(): Promise<Snippet[]> {
  return invoke("get_snippets");
}

export async function createSnippet(trigger: string, expansion: string): Promise<Snippet> {
  return invoke("create_snippet", { trigger, expansion });
}

export async function updateSnippet(id: string, trigger: string, expansion: string): Promise<void> {
  return invoke("update_snippet", { id, trigger, expansion });
}

export async function deleteSnippet(id: string): Promise<void> {
  return invoke("delete_snippet", { id });
}

// ============ Dictionary Types & Commands ============

export interface DictionaryWord {
  id: string;
  word: string;
  phonetic: string | null;
  category: string;
  is_auto_learned: boolean;
  created_at: string;
  use_count: number;
}

export async function getDictionaryWords(): Promise<DictionaryWord[]> {
  return invoke("get_dictionary_words");
}

export async function addDictionaryWord(word: string, isAutoLearned?: boolean): Promise<DictionaryWord> {
  return invoke("add_dictionary_word", { word, isAutoLearned });
}

export async function updateDictionaryWord(id: string, word: string, phonetic?: string): Promise<void> {
  return invoke("update_dictionary_word", { id, word, phonetic });
}

export async function deleteDictionaryWord(id: string): Promise<void> {
  return invoke("delete_dictionary_word", { id });
}

// ============ Error Log Types & Commands ============

export type ErrorType = "Transcription" | "LLMProcessing" | "ActionExecution" | "AudioCapture" | "Network" | "RateLimit" | "Unknown";

export interface ErrorEntry {
  id: string;
  error_type: ErrorType;
  message: string;
  details: string | null;
  timestamp: string;
  dismissed: boolean;
}

export async function getErrors(limit?: number): Promise<ErrorEntry[]> {
  return invoke("get_errors", { limit });
}

export async function getUndismissedErrors(): Promise<ErrorEntry[]> {
  return invoke("get_undismissed_errors");
}

export async function dismissError(id: string): Promise<boolean> {
  return invoke("dismiss_error", { id });
}

export async function dismissAllErrors(): Promise<void> {
  return invoke("dismiss_all_errors");
}

// ============ Correction Learning Commands ============

export async function learnCorrection(correctedText: string): Promise<string[]> {
  return invoke("learn_correction", { correctedText });
}

// ============ Utility ============

export function isElectron(): boolean {
  return typeof window !== "undefined" && Boolean(window.listenOS);
}
