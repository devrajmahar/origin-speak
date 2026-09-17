import { useState, useCallback, useEffect } from "react";
import { motion, AnimatePresence } from "framer-motion";
import * as Select from "@/components/ui/select";
import { cn } from "@/lib/utils";
import {
  isElectron,
  getAssistantHotkey,
  getAutostartEnabled,
  setAutostartEnabled,
  getTriggerHotkey,
  setAssistantHotkey,
  setTriggerHotkey,
  getLanguagePreferences,
  setLanguagePreferences,
  getVibeCodingConfig,
  setVibeCodingConfig,
  listLocalModels,
  getTranscriptionSettings,
  setTranscriptionModel,
  getTranscriptionRuntimeStatus,
  downloadLocalModel,
} from "@/lib/desktop";
import type { LocalModelInfo, TranscriptionRuntimeStatus, VibeCodingConfig } from "@/lib/desktop";
import { checkForUpdates } from "@/lib/updater";
import { useSettings } from "@/context/SettingsContext";
import packageInfo from "../../../package.json";
import {
  Settings02Icon,
  ComputerIcon,
  HashtagIcon,
  TestTubeIcon,
  UserCircleIcon,
  UserGroupIcon,
  CreditCardIcon,
  ShieldUserIcon,
  Cancel01Icon,
  Download04Icon,
  Loading03Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
}

type SettingsSection =
  | "general"
  | "system"
  | "vibe-coding"
  | "experimental"
  | "account"
  | "team"
  | "billing"
  | "privacy";

interface SettingsNavItem {
  id: SettingsSection;
  label: string;
  icon: typeof Settings02Icon;
  category: "settings" | "account";
}

const settingsNavItems: SettingsNavItem[] = [
  { id: "general", label: "General", icon: Settings02Icon, category: "settings" },
  { id: "system", label: "System", icon: ComputerIcon, category: "settings" },
  { id: "vibe-coding", label: "Vibe coding", icon: HashtagIcon, category: "settings" },
  { id: "experimental", label: "Experimental", icon: TestTubeIcon, category: "settings" },
  { id: "account", label: "Account", icon: UserCircleIcon, category: "account" },
  { id: "team", label: "Team", icon: UserGroupIcon, category: "account" },
  { id: "billing", label: "Plans and Billing", icon: CreditCardIcon, category: "account" },
  { id: "privacy", label: "Data and Privacy", icon: ShieldUserIcon, category: "account" },
];

const APP_VERSION = packageInfo.version;
type ShortcutTarget = "pushToTalk" | "assistant";

const SOURCE_LANGUAGE_OPTIONS = [
  { value: "auto", label: "Auto Detect" },
  { value: "en", label: "English" },
  { value: "hi", label: "Hindi" },
  { value: "es", label: "Spanish" },
  { value: "fr", label: "French" },
  { value: "de", label: "German" },
  { value: "it", label: "Italian" },
  { value: "pt", label: "Portuguese" },
  { value: "ru", label: "Russian" },
  { value: "zh", label: "Chinese" },
  { value: "ja", label: "Japanese" },
  { value: "ko", label: "Korean" },
  { value: "ar", label: "Arabic" },
];

const TARGET_LANGUAGE_OPTIONS = SOURCE_LANGUAGE_OPTIONS.filter((option) => option.value !== "auto");

const VIBE_ACTIVATION_OPTIONS: Array<{
  value: VibeCodingConfig["activation_mode"];
  label: string;
  description: string;
}> = [
  { value: "SmartAuto", label: "Dynamic smart", description: "Auto-enhance only when coding intent is strong." },
  { value: "ManualOnly", label: "Manual trigger", description: "Enhance only when you start with the trigger phrase." },
  { value: "Always", label: "Always on", description: "Enhance every typed dictation prompt." },
];

const VIBE_TARGET_TOOL_OPTIONS: Array<{
  value: VibeCodingConfig["target_tool"];
  label: string;
}> = [
  { value: "Generic", label: "Generic" },
  { value: "Cursor", label: "Cursor" },
  { value: "Windsurf", label: "Windsurf" },
  { value: "Claude", label: "Claude" },
  { value: "ChatGPT", label: "ChatGPT" },
  { value: "Copilot", label: "GitHub Copilot" },
];

const VIBE_DETAIL_LEVEL_OPTIONS: Array<{
  value: VibeCodingConfig["detail_level"];
  label: string;
}> = [
  { value: "Concise", label: "Concise" },
  { value: "Balanced", label: "Balanced" },
  { value: "Detailed", label: "Detailed" },
];

type SettingsSelectOption = {
  value: string;
  label: string;
};

const DEFAULT_VIBE_CONFIG: VibeCodingConfig = {
  enabled: false,
  activation_mode: "SmartAuto",
  trigger_phrase: "vibe",
  target_tool: "Generic",
  detail_level: "Balanced",
  include_constraints: true,
  include_acceptance_criteria: true,
  include_test_notes: false,
  concise_output: false,
};

function modelLabel(model: LocalModelInfo): string {
  return model.label || model.id;
}

function selectedTranscriptionModel(settings: { model: string } | null): string {
  return settings?.model || "";
}

function englishOnlyModelWarning(model: string, language: string): string | null {
  const normalizedLanguage = language.trim().toLowerCase();
  if (!model.endsWith(".en") || !normalizedLanguage || normalizedLanguage === "auto" || normalizedLanguage.startsWith("en")) {
    return null;
  }

  const multilingualModel = model.replace(/\.en$/, "");
  return `${model} is English-only. Select the multilingual ${multilingualModel} model in System before dictating in ${getLanguageLabel(language, true)}.`;
}

function getLanguageLabel(code: string, source = false): string {
  const options = source ? SOURCE_LANGUAGE_OPTIONS : TARGET_LANGUAGE_OPTIONS;
  return options.find((option) => option.value === code)?.label ?? code;
}

function getVibeActivationDescription(mode: VibeCodingConfig["activation_mode"]): string {
  return (
    VIBE_ACTIVATION_OPTIONS.find((option) => option.value === mode)?.description ??
    "Auto-enhance prompts."
  );
}

export function SettingsModal({ isOpen, onClose }: SettingsModalProps) {
  const [activeSection, setActiveSection] = useState<SettingsSection>("general");

  const settingsItems = settingsNavItems.filter((item) => item.category === "settings");
  const accountItems = settingsNavItems.filter((item) => item.category === "account");

  return (
    <AnimatePresence>
      {isOpen && (
        <>
          {/* Backdrop */}
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            onClick={onClose}
            className="fixed inset-0 z-50 bg-overlay backdrop-blur-sm"
          />

          {/* Modal */}
          <motion.div
            initial={{ opacity: 0, scale: 0.95 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.95 }}
            transition={{ duration: 0.2 }}
            className="fixed left-1/2 top-1/2 z-50 flex h-[600px] w-[800px] -translate-x-1/2 -translate-y-1/2 overflow-hidden rounded-df bg-card shadow-2xl"
          >
            {/* Settings Sidebar */}
            <div className="flex w-56 flex-col border-r border-muted-border bg-muted p-4">
              {/* Settings Section */}
              <div className="mb-4">
                <h3 className="mb-2 px-3 text-xs font-normal uppercase tracking-wider text-muted-foreground">
                  Settings
                </h3>
                <ul className="space-y-1">
                  {settingsItems.map((item) => (
                    <li key={item.id}>
                      <button
                        onClick={() => setActiveSection(item.id)}
                        className={cn(
                          "flex w-full items-center gap-3 rounded-df px-3 py-2 text-sm font-normal transition-colors",
                          activeSection === item.id
                            ? "bg-accent text-foreground"
                            : "text-muted-foreground hover:bg-accent hover:text-foreground"
                        )}
                      >
                        <HugeiconsIcon icon={item.icon} size={18} />
                        {item.label}
                      </button>
                    </li>
                  ))}
                </ul>
              </div>

              {/* Account Section */}
              <div>
                <h3 className="mb-2 px-3 text-xs font-normal uppercase tracking-wider text-muted-foreground">
                  Account
                </h3>
                <ul className="space-y-1">
                  {accountItems.map((item) => (
                    <li key={item.id}>
                      <button
                        onClick={() => setActiveSection(item.id)}
                        className={cn(
                          "flex w-full items-center gap-3 rounded-df px-3 py-2 text-sm font-normal transition-colors",
                          activeSection === item.id
                            ? "bg-accent text-foreground"
                            : "text-muted-foreground hover:bg-accent hover:text-foreground"
                        )}
                      >
                        <HugeiconsIcon icon={item.icon} size={18} />
                        {item.label}
                      </button>
                    </li>
                  ))}
                </ul>
              </div>

              {/* Version */}
              <div className="mt-auto px-3 pt-4">
                <span className="text-xs text-muted-foreground">ListenOS v{APP_VERSION}</span>
              </div>
            </div>

            {/* Content Area */}
            <div className="flex-1 overflow-y-auto p-6">
              {/* Close Button */}
              <button
                onClick={onClose}
                className="ui-button ui-button-ghost absolute right-4 top-4 rounded-df p-2 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              >
                <HugeiconsIcon icon={Cancel01Icon} size={20} />
              </button>

              {/* Section Content */}
              <SettingsContent section={activeSection} />
            </div>
          </motion.div>
        </>
      )}
    </AnimatePresence>
  );
}

function SettingsContent({ section }: { section: SettingsSection }) {
  const [isCheckingUpdates, setIsCheckingUpdates] = useState(false);
  const [updateStatus, setUpdateStatus] = useState<string | null>(null);
  const [recordingShortcutTarget, setRecordingShortcutTarget] = useState<ShortcutTarget | null>(
    null
  );
  const [currentShortcut, setCurrentShortcut] = useState("Ctrl+Space");
  const [currentAssistantShortcut, setCurrentAssistantShortcut] = useState("Ctrl+Alt+Space");
  const { settings, updateSettings } = useSettings();

  // Local state for system settings
  const [startOnLogin, setStartOnLogin] = useState(settings.startOnLogin ?? false);
  const [showInTray, setShowInTray] = useState(settings.showInTray ?? true);
  const [sourceLanguage, setSourceLanguage] = useState("en");
  const [targetLanguage, setTargetLanguage] = useState(settings.language ?? "en");
  const [autostartLoading, setAutostartLoading] = useState(false);
  const [vibeConfig, setVibeConfigState] = useState<VibeCodingConfig>(DEFAULT_VIBE_CONFIG);
  const [vibeSaving, setVibeSaving] = useState(false);
  const [vibeLoading, setVibeLoading] = useState(false);
  const [localModels, setLocalModels] = useState<LocalModelInfo[]>([]);
  const [selectedModel, setSelectedModel] = useState("");
  const [transcriptionStatus, setTranscriptionStatus] = useState<TranscriptionRuntimeStatus | null>(null);
  const [modelSettingsLoading, setModelSettingsLoading] = useState(false);
  const [modelSettingsSaving, setModelSettingsSaving] = useState(false);
  const [downloadingModel, setDownloadingModel] = useState<string | null>(null);
  const [modelNotice, setModelNotice] = useState<string | null>(null);

  // Load actual autostart state on mount
  useEffect(() => {
    if (isElectron()) {
      getAutostartEnabled()
        .then((enabled) => setStartOnLogin(enabled))
        .catch((err) => console.error("Failed to get autostart status:", err));

      getTriggerHotkey()
        .then((hotkey) => setCurrentShortcut(hotkey))
        .catch((err) => console.error("Failed to get current hotkey:", err));

      getAssistantHotkey()
        .then((hotkey) => setCurrentAssistantShortcut(hotkey))
        .catch((err) => console.error("Failed to get assistant hotkey:", err));

      getLanguagePreferences()
        .then((prefs) => {
          setSourceLanguage(prefs.source_language);
          setTargetLanguage(prefs.target_language);
        })
        .catch((err) => console.error("Failed to get language preferences:", err));

      setVibeLoading(true);
      getVibeCodingConfig()
        .then((config) => setVibeConfigState(config))
        .catch((err) => {
          console.error("Failed to load vibe coding config:", err);
          setUpdateStatus("Failed to load vibe coding settings");
        })
        .finally(() => setVibeLoading(false));

      setModelSettingsLoading(true);
      Promise.all([
        listLocalModels(),
        getTranscriptionSettings(),
        getTranscriptionRuntimeStatus(),
      ])
        .then(([models, transcriptionSettings, runtimeStatus]) => {
          setLocalModels(models);
          setSelectedModel(selectedTranscriptionModel(transcriptionSettings));
          setTranscriptionStatus(runtimeStatus);
        })
        .catch((err) => {
          console.error("Failed to load local transcription settings:", err);
          setModelNotice(err instanceof Error ? err.message : "Failed to load local transcription settings");
        })
        .finally(() => setModelSettingsLoading(false));
    }
  }, []);

  // Update local state when settings change
  useEffect(() => {
    setShowInTray(settings.showInTray);
    if (!isElectron()) {
      setTargetLanguage(settings.language);
    }
    if (settings.hotkey) {
      setCurrentShortcut(settings.hotkey);
    }
    if (settings.assistantHotkey) {
      setCurrentAssistantShortcut(settings.assistantHotkey);
    }
  }, [settings]);

  const handleCheckUpdates = useCallback(async () => {
    if (!isElectron()) {
      setUpdateStatus("Updates only available in desktop app");
      return;
    }
    
    setIsCheckingUpdates(true);
    setUpdateStatus(null);
    
    try {
      await checkForUpdates(false);
      setUpdateStatus("You're on the latest version!");
    } catch (error) {
      console.error("Update check failed:", error);
      setUpdateStatus("Failed to check for updates");
    } finally {
      setIsCheckingUpdates(false);
    }
  }, []);

  const handleStartOnLoginChange = useCallback(async (checked: boolean) => {
    if (!isElectron()) {
      setStartOnLogin(checked);
      await updateSettings({ startOnLogin: checked });
      return;
    }
    
    setAutostartLoading(true);
    try {
      const newState = await setAutostartEnabled(checked);
      setStartOnLogin(newState);
      await updateSettings({ startOnLogin: newState });
    } catch (err) {
      console.error("Failed to set autostart:", err);
      // Revert UI state on error
      setStartOnLogin(!checked);
    } finally {
      setAutostartLoading(false);
    }
  }, [updateSettings]);

  const handleShowInTrayChange = useCallback(async (checked: boolean) => {
    setShowInTray(checked);
    await updateSettings({ showInTray: checked });
  }, [updateSettings]);

  const refreshTranscriptionStatus = useCallback(async () => {
    if (!isElectron()) return;
    const [models, runtimeStatus] = await Promise.all([
      listLocalModels(),
      getTranscriptionRuntimeStatus(),
    ]);
    setLocalModels(models);
    setTranscriptionStatus(runtimeStatus);
  }, []);

  const handleTranscriptionModelChange = useCallback(async (model: string) => {
    if (!isElectron()) return;
    const previous = selectedModel;
    setSelectedModel(model);
    const target = localModels.find((candidate) => candidate.id === model);
    if (target && !target.downloaded) {
      setModelNotice(`${target.label} must be downloaded before it becomes the active model.`);
      return;
    }

    setModelSettingsSaving(true);
    setModelNotice(null);
    try {
      const saved = await setTranscriptionModel(model);
      setSelectedModel(selectedTranscriptionModel(saved) || model);
      await refreshTranscriptionStatus();
      setModelNotice("Local transcription model updated");
    } catch (err) {
      console.error("Failed to update transcription model:", err);
      setSelectedModel(previous);
      setModelNotice(err instanceof Error ? err.message : "Failed to update transcription model");
    } finally {
      setModelSettingsSaving(false);
    }
  }, [localModels, refreshTranscriptionStatus, selectedModel]);

  const handleModelDownload = useCallback(async (model: string) => {
    if (!isElectron()) return;
    setDownloadingModel(model);
    setModelNotice(`Downloading ${model}...`);
    try {
      await downloadLocalModel(model);
    } catch (err) {
      console.error("Failed to download local model:", err);
      setModelNotice(err instanceof Error ? err.message : "Failed to download local model");
      setDownloadingModel(null);
      return;
    }

    try {
      const saved = await setTranscriptionModel(model);
      setSelectedModel(selectedTranscriptionModel(saved) || model);
      await refreshTranscriptionStatus();
      setModelNotice(`${model} is ready to use`);
    } catch (err) {
      console.error("Model downloaded but could not be activated:", err);
      await refreshTranscriptionStatus().catch(() => undefined);
      setModelNotice(err instanceof Error
        ? `Model downloaded, but activation failed: ${err.message}`
        : "Model downloaded, but activation failed. Select it again to retry.");
    }
    setDownloadingModel(null);
  }, [refreshTranscriptionStatus]);

  const handleSourceLanguageChange = useCallback(async (newLanguage: string) => {
    const previousSource = sourceLanguage;
    const previousTarget = targetLanguage;
    setSourceLanguage(newLanguage);

    if (isElectron()) {
      try {
        const updated = await setLanguagePreferences(newLanguage, targetLanguage);
        setSourceLanguage(updated.source_language);
        setTargetLanguage(updated.target_language);
      } catch (err) {
        console.error("Failed to set source language:", err);
        setSourceLanguage(previousSource);
        setTargetLanguage(previousTarget);
        setUpdateStatus("Failed to update source language");
      }
      return;
    }
  }, [sourceLanguage, targetLanguage]);

  const handleTargetLanguageChange = useCallback(async (newLanguage: string) => {
    const previousSource = sourceLanguage;
    const previousTarget = targetLanguage;
    setTargetLanguage(newLanguage);

    if (isElectron()) {
      try {
        const updated = await setLanguagePreferences(sourceLanguage, newLanguage);
        setSourceLanguage(updated.source_language);
        setTargetLanguage(updated.target_language);
        await updateSettings({ language: updated.target_language });
      } catch (err) {
        console.error("Failed to set target language:", err);
        setSourceLanguage(previousSource);
        setTargetLanguage(previousTarget);
        setUpdateStatus("Failed to update target language");
      }
      return;
    }
    try {
      await updateSettings({ language: newLanguage });
    } catch (err) {
      console.error("Failed to sync target language setting:", err);
    }
  }, [sourceLanguage, targetLanguage, updateSettings]);

  const applyVibeConfig = useCallback(async (patch: Partial<VibeCodingConfig>) => {
    const previous = vibeConfig;
    const trigger = (patch.trigger_phrase ?? previous.trigger_phrase).trim().toLowerCase();
    const next: VibeCodingConfig = {
      ...previous,
      ...patch,
      trigger_phrase: trigger.length > 0 ? trigger : "vibe",
    };

    setVibeConfigState(next);

    if (!isElectron()) {
      return;
    }

    setVibeSaving(true);
    try {
      const saved = await setVibeCodingConfig(next);
      setVibeConfigState(saved);
    } catch (err) {
      console.error("Failed to save vibe coding config:", err);
      setVibeConfigState(previous);
      setUpdateStatus("Failed to update vibe coding settings");
    } finally {
      setVibeSaving(false);
    }
  }, [vibeConfig]);

  const applyShortcut = useCallback(async (target: ShortcutTarget, rawShortcut: string) => {
    if (isElectron()) {
      if (target === "assistant") {
        const normalized = await setAssistantHotkey(rawShortcut);
        setCurrentAssistantShortcut(normalized);
        await updateSettings({ assistantHotkey: normalized });
      } else {
        const normalized = await setTriggerHotkey(rawShortcut);
        setCurrentShortcut(normalized);
        await updateSettings({ hotkey: normalized });
      }
      return;
    }

    if (target === "assistant") {
      setCurrentAssistantShortcut(rawShortcut);
      await updateSettings({ assistantHotkey: rawShortcut });
      return;
    }
    setCurrentShortcut(rawShortcut);
    await updateSettings({ hotkey: rawShortcut });
  }, [updateSettings]);

  const handleShortcutRecord = useCallback((target: ShortcutTarget) => {
    setRecordingShortcutTarget(target);
    
    const handleKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      const keys: string[] = [];
      if (e.ctrlKey) keys.push("Ctrl");
      if (e.altKey) keys.push("Alt");
      if (e.shiftKey) keys.push("Shift");
      if (e.metaKey) keys.push("Win");
      
      // Add the actual key if it's not a modifier
      if (!['Control', 'Alt', 'Shift', 'Meta'].includes(e.key)) {
        const keyMap: Record<string, string> = {
          " ": "Space",
          ArrowUp: "Up",
          ArrowDown: "Down",
          ArrowLeft: "Left",
          ArrowRight: "Right",
          Escape: "Esc",
          Enter: "Enter",
          Tab: "Tab",
        };
        const normalizedKey = keyMap[e.key] ||
          (e.key.length === 1
            ? e.key.toUpperCase()
            : e.key.charAt(0).toUpperCase() + e.key.slice(1));
        keys.push(normalizedKey);
      }
      
      if (keys.length >= 2) {
        const shortcut = keys.join('+');
        setRecordingShortcutTarget(null);
        document.removeEventListener('keydown', handleKeyDown);

        void applyShortcut(target, shortcut).catch((err) => {
          console.error("Failed to apply shortcut:", err);
          setUpdateStatus("Failed to update shortcut");
        });
      }
    };
    
    document.addEventListener('keydown', handleKeyDown);
    
    // Cancel after 5 seconds
    setTimeout(() => {
      setRecordingShortcutTarget(null);
      document.removeEventListener('keydown', handleKeyDown);
    }, 5000);
  }, [applyShortcut]);

  switch (section) {
    case "general": {
      const sourceModelWarning = englishOnlyModelWarning(selectedModel, sourceLanguage);
      return (
        <div className="animate-fade-in">
          <h2 className="mb-6 text-2xl font-normal text-foreground">General</h2>
          <div className="space-y-6">
            <SettingsRow
              label="Hold-to-talk shortcut"
              description={
                recordingShortcutTarget === "pushToTalk"
                  ? "Press your shortcut..."
                  : `Hold ${currentShortcut} and speak.`
              }
              action={
                <button 
                  onClick={() => handleShortcutRecord("pushToTalk")}
                  className={cn(
                    "ui-button rounded-df border px-4 py-2 text-sm font-normal transition-colors cursor-pointer",
                    recordingShortcutTarget === "pushToTalk"
                      ? "border-primary bg-primary/10 text-primary animate-pulse" 
                      : "border-muted-border bg-muted text-foreground hover:bg-accent"
                  )}
                >
                  {recordingShortcutTarget === "pushToTalk" ? "Recording..." : "Change"}
                </button>
              }
            />
            <SettingsRow
              label="Assistant mode shortcut"
              description={
                recordingShortcutTarget === "assistant"
                  ? "Press your shortcut..."
                  : `Press ${currentAssistantShortcut} to toggle idle assistant listening.`
              }
              action={
                <button
                  onClick={() => handleShortcutRecord("assistant")}
                  className={cn(
                    "ui-button rounded-df border px-4 py-2 text-sm font-normal transition-colors cursor-pointer",
                    recordingShortcutTarget === "assistant"
                      ? "border-primary bg-primary/10 text-primary animate-pulse"
                      : "border-muted-border bg-muted text-foreground hover:bg-accent"
                  )}
                >
                  {recordingShortcutTarget === "assistant" ? "Recording..." : "Change"}
                </button>
              }
            />
            <SettingsRow
              label="Microphone"
              description="Auto-detect (Default)"
              action={
                <button className="ui-button ui-button-outline rounded-df border border-muted-border bg-muted px-4 py-2 text-sm font-normal text-foreground transition-colors hover:bg-accent">
                  Change
                </button>
              }
            />
            <SettingsRow
              label="Source language"
              description={sourceModelWarning || getLanguageLabel(sourceLanguage, true)}
              action={
                <SettingsSelect
                  value={sourceLanguage}
                  onValueChange={(value) =>
                    void handleSourceLanguageChange(value)
                  }
                  options={SOURCE_LANGUAGE_OPTIONS}
                />
              }
            />
            <SettingsRow
              label="Target language"
              description={getLanguageLabel(targetLanguage)}
              action={
                <SettingsSelect
                  value={targetLanguage}
                  onValueChange={(value) =>
                    void handleTargetLanguageChange(value)
                  }
                  options={TARGET_LANGUAGE_OPTIONS}
                />
              }
            />
            <SettingsRow
              label="Check for updates"
              description={updateStatus || "Check if a new version is available"}
              action={
                <button 
                  onClick={handleCheckUpdates}
                  disabled={isCheckingUpdates}
                  className="ui-button ui-button-outline flex items-center gap-2 rounded-df border border-muted-border bg-muted px-4 py-2 text-sm font-normal text-foreground transition-colors hover:bg-accent"
                >
                  <HugeiconsIcon 
                    icon={isCheckingUpdates ? Loading03Icon : Download04Icon} 
                    size={16} 
                    className={isCheckingUpdates ? "animate-spin" : ""}
                  />
                  {isCheckingUpdates ? "Checking..." : "Check now"}
                </button>
              }
            />
          </div>
        </div>
      );
    }
    case "system": {
      const selectedModelInfo = localModels.find((model) => model.id === selectedModel);
      const modelDescription = selectedModelInfo
        ? `${selectedModelInfo.filename} · ${selectedModelInfo.downloaded ? "Downloaded" : "Download required"}`
        : modelSettingsLoading
          ? "Loading local models..."
          : "Choose the local speech model used for dictation.";
      const runtimeDescription = transcriptionStatus
        ? modelNotice || transcriptionStatus.last_error || (transcriptionStatus.phase === "Ready"
          ? `Ready · ${transcriptionStatus.model}`
          : transcriptionStatus.phase === "ModelMissing"
            ? `Model ${transcriptionStatus.model} needs to be downloaded.`
            : transcriptionStatus.phase)
        : modelNotice || "Runtime status is not available yet.";

      return (
        <div className="animate-fade-in">
          <h2 className="mb-6 text-2xl font-normal text-foreground">System</h2>
          <div className="space-y-6">
            <SettingsRow
              label="Start on login"
              description="Automatically start ListenOS when you log in"
              action={
                <ToggleSwitch 
                  checked={startOnLogin} 
                  onChange={handleStartOnLoginChange}
                  disabled={autostartLoading}
                />
              }
            />
            <SettingsRow
              label="Show in menu bar"
              description="Display ListenOS icon in the system tray"
              action={
                <ToggleSwitch 
                  checked={showInTray} 
                  onChange={handleShowInTrayChange}
                />
              }
            />
            <SettingsRow
              label="Transcription model"
              description={modelDescription}
              action={
                <div className="flex items-center gap-2">
                  {localModels.length > 0 ? (
                    <SettingsSelect
                      value={selectedModel || localModels[0].id}
                      onValueChange={(value) => void handleTranscriptionModelChange(value)}
                      disabled={modelSettingsLoading || modelSettingsSaving || downloadingModel !== null}
                      options={localModels.map((model) => ({
                        value: model.id,
                        label: `${modelLabel(model)}${model.downloaded ? "" : " · download"}`,
                      }))}
                      className="min-w-[210px]"
                    />
                  ) : (
                    <span className="text-sm text-muted-foreground">
                      {modelSettingsLoading ? "Loading..." : "No models available"}
                    </span>
                  )}
                  {selectedModelInfo && !selectedModelInfo.downloaded && (
                    <button
                      onClick={() => void handleModelDownload(selectedModelInfo.id)}
                      disabled={downloadingModel !== null || modelSettingsSaving}
                      className="ui-button ui-button-outline flex items-center gap-2 rounded-df border border-muted-border bg-muted px-4 py-2 text-sm font-normal text-foreground transition-colors hover:bg-accent disabled:opacity-50"
                    >
                      <HugeiconsIcon
                        icon={downloadingModel === selectedModelInfo.id ? Loading03Icon : Download04Icon}
                        size={16}
                        className={downloadingModel === selectedModelInfo.id ? "animate-spin" : ""}
                      />
                      {downloadingModel === selectedModelInfo.id ? "Downloading..." : "Download"}
                    </button>
                  )}
                </div>
              }
            />
            <SettingsRow
              label="Local transcription runtime"
              description={runtimeDescription}
              action={
                <span className={cn(
                  "text-sm",
                  transcriptionStatus?.phase === "Ready" ? "text-positive" : "text-muted-foreground"
                )}>
                  {transcriptionStatus?.phase || "Checking"}
                </span>
              }
            />
          </div>
        </div>
      );
    }
    case "vibe-coding":
      return (
        <div className="animate-fade-in">
          <h2 className="mb-6 text-2xl font-normal text-foreground">Vibe coding</h2>
          <p className="mb-4 text-muted-foreground">
            Turn rough spoken coding ideas into clean, structured prompts before they are pasted.
          </p>
          <div className="space-y-6">
            <SettingsRow
              label="Enable vibe coding"
              description="Rewrite voice dictation into stronger AI coding prompts."
              action={
                <ToggleSwitch
                  checked={vibeConfig.enabled}
                  onChange={(checked) => void applyVibeConfig({ enabled: checked })}
                  disabled={vibeSaving || vibeLoading}
                />
              }
            />
            <SettingsRow
              label="Activation mode"
              description={getVibeActivationDescription(vibeConfig.activation_mode)}
              action={
                <SettingsSelect
                  value={vibeConfig.activation_mode}
                  onValueChange={(value) =>
                    void applyVibeConfig({
                      activation_mode:
                        value as VibeCodingConfig["activation_mode"],
                    })
                  }
                  disabled={!vibeConfig.enabled || vibeSaving || vibeLoading}
                  options={VIBE_ACTIVATION_OPTIONS}
                />
              }
            />
            <SettingsRow
              label="Trigger phrase"
              description={`Say this first in manual mode. Example: "${vibeConfig.trigger_phrase} build an Electron IPC command..."`}
              action={
                <input
                  value={vibeConfig.trigger_phrase}
                  onChange={(e) => setVibeConfigState((prev) => ({ ...prev, trigger_phrase: e.target.value }))}
                  onBlur={(e) => void applyVibeConfig({ trigger_phrase: e.target.value })}
                  disabled={!vibeConfig.enabled || vibeSaving || vibeLoading}
                  className="ui-input w-44 rounded-df border border-muted-border bg-input px-3 py-2 text-sm font-normal text-foreground transition-colors"
                  placeholder="vibe"
                />
              }
            />
            <SettingsRow
              label="Target AI tool"
              description="Tune wording for your primary coding assistant."
              action={
                <SettingsSelect
                  value={vibeConfig.target_tool}
                  onValueChange={(value) =>
                    void applyVibeConfig({
                      target_tool: value as VibeCodingConfig["target_tool"],
                    })
                  }
                  disabled={!vibeConfig.enabled || vibeSaving || vibeLoading}
                  options={VIBE_TARGET_TOOL_OPTIONS}
                />
              }
            />
            <SettingsRow
              label="Prompt detail level"
              description="Control how concise or detailed rewritten prompts should be."
              action={
                <SettingsSelect
                  value={vibeConfig.detail_level}
                  onValueChange={(value) =>
                    void applyVibeConfig({
                      detail_level: value as VibeCodingConfig["detail_level"],
                    })
                  }
                  disabled={!vibeConfig.enabled || vibeSaving || vibeLoading}
                  options={VIBE_DETAIL_LEVEL_OPTIONS}
                />
              }
            />
            <SettingsRow
              label="Include constraints"
              description="Ask AI to keep boundaries explicit (stack, scope, limits)."
              action={
                <ToggleSwitch
                  checked={vibeConfig.include_constraints}
                  onChange={(checked) => void applyVibeConfig({ include_constraints: checked })}
                  disabled={!vibeConfig.enabled || vibeSaving || vibeLoading}
                />
              }
            />
            <SettingsRow
              label="Include acceptance criteria"
              description="Add clear completion criteria for better first-pass output."
              action={
                <ToggleSwitch
                  checked={vibeConfig.include_acceptance_criteria}
                  onChange={(checked) => void applyVibeConfig({ include_acceptance_criteria: checked })}
                  disabled={!vibeConfig.enabled || vibeSaving || vibeLoading}
                />
              }
            />
            <SettingsRow
              label="Include test checklist"
              description="Add test and verification notes to improve reliability."
              action={
                <ToggleSwitch
                  checked={vibeConfig.include_test_notes}
                  onChange={(checked) => void applyVibeConfig({ include_test_notes: checked })}
                  disabled={!vibeConfig.enabled || vibeSaving || vibeLoading}
                />
              }
            />
            <SettingsRow
              label="Concise output bias"
              description="Prefer shorter prompts for rapid AI iteration."
              action={
                <ToggleSwitch
                  checked={vibeConfig.concise_output}
                  onChange={(checked) => void applyVibeConfig({ concise_output: checked })}
                  disabled={!vibeConfig.enabled || vibeSaving || vibeLoading}
                />
              }
            />

            <div className="rounded-df border border-muted-border bg-muted p-4">
              <p className="mb-1 text-sm font-normal text-foreground">Live behavior</p>
              <p className="text-sm text-muted-foreground">
                Spoken coding requests are rewritten before paste. Commands like opening apps or taking screenshots
                still execute normally and skip vibe rewriting.
              </p>
              {(vibeSaving || vibeLoading) && (
                <p className="mt-2 text-xs text-muted-foreground">Syncing settings...</p>
              )}
            </div>
          </div>
        </div>
      );
    case "experimental":
      return (
        <div className="animate-fade-in">
          <h2 className="mb-6 text-2xl font-normal text-foreground">Experimental</h2>
          <p className="text-muted-foreground mb-4">Try out new features before they&apos;re released.</p>
          <div className="rounded-df border border-muted-border bg-muted p-4">
            <p className="text-sm text-muted-foreground">No experimental features available at this time.</p>
          </div>
        </div>
      );
    case "account":
      return (
        <div className="animate-fade-in">
          <h2 className="mb-6 text-2xl font-normal text-foreground">Account</h2>
          <div className="space-y-6">
            <SettingsRow
              label="Mode"
              description="Self-hosted local mode. No sign-in required."
              action={<span className="text-sm text-positive">Local only</span>}
            />
            <SettingsRow
              label="Profile"
              description="Local user"
              action={<span className="text-sm text-muted-foreground">Stored on this device</span>}
            />
          </div>
        </div>
      );
    case "team":
      return (
        <div className="animate-fade-in">
          <h2 className="mb-6 text-2xl font-normal text-foreground">Team</h2>
          <div className="rounded-df border border-muted-border bg-muted p-4">
            <p className="text-sm text-muted-foreground">Team sync is disabled in self-hosted mode.</p>
          </div>
        </div>
      );
    case "billing":
      return (
        <div className="animate-fade-in">
          <h2 className="mb-6 text-2xl font-normal text-foreground">Plans and Billing</h2>
          <div className="rounded-df border border-muted-border bg-muted p-4">
            <p className="text-sm text-muted-foreground">
              ListenOS local mode has no account billing. Speech models are stored and run on this device.
            </p>
          </div>
        </div>
      );
    case "privacy":
      return (
        <div className="animate-fade-in">
          <h2 className="mb-6 text-2xl font-normal text-foreground">Data and Privacy</h2>
          <div className="space-y-6">
            <SettingsRow
              label="Voice data"
              description="Voice recordings are processed locally and not stored"
              action={<span className="text-sm text-positive">Secure</span>}
            />
            <SettingsRow
              label="Command history"
              description="Clear your command history from this device"
              action={
                <button className="ui-button rounded-df border border-negative/25 bg-negative/10 px-4 py-2 text-sm font-normal text-negative transition-colors hover:bg-accent">
                  Clear history
                </button>
              }
            />
            <SettingsRow
              label="Delete local data"
              description="Clear local ListenOS data on this device"
              action={
                <button className="ui-button rounded-df border border-negative/25 bg-negative/10 px-4 py-2 text-sm font-normal text-negative transition-colors hover:bg-accent">
                  Clear local data
                </button>
              }
            />
          </div>
        </div>
      );
    default:
      return null;
  }
}

function SettingsRow({
  label,
  description,
  action,
}: {
  label: string;
  description: string;
  action: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between border-b border-border pb-4">
      <div>
        <h3 className="text-sm font-normal text-foreground">{label}</h3>
        <p className="text-sm text-muted-foreground">{description}</p>
      </div>
      {action}
    </div>
  );
}

function SettingsSelect({
  value,
  onValueChange,
  options,
  disabled,
  className,
}: {
  value: string;
  onValueChange: (value: string) => void;
  options: ReadonlyArray<SettingsSelectOption>;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <Select.Root
      value={value}
      onValueChange={onValueChange}
      disabled={disabled}
      size="small"
      variant="compact"
    >
      <Select.Trigger className={cn("min-w-[180px] bg-card", className)}>
        <Select.Value />
      </Select.Trigger>
      <Select.Content>
        {options.map((option) => (
          <Select.Item key={option.value} value={option.value}>
            {option.label}
          </Select.Item>
        ))}
      </Select.Content>
    </Select.Root>
  );
}

function ToggleSwitch({ 
  checked, 
  onChange,
  disabled = false,
}: { 
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <button
      onClick={() => !disabled && onChange(!checked)}
      disabled={disabled}
      aria-pressed={checked}
      className={cn(
        "relative h-6 w-11 rounded-lg border border-muted-border transition-colors",
        checked ? "bg-primary" : "bg-muted",
        disabled && "cursor-not-allowed"
      )}
    >
        <span
          className={cn(
            "absolute left-0.5 top-0.5 h-5 w-5 rounded-lg bg-card shadow transition-transform",
            checked && "translate-x-5"
          )}
        />
    </button>
  );
}

