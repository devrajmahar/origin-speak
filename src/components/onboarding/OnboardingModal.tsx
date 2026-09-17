import { useState, useEffect, useCallback } from "react";
import { motion, AnimatePresence } from "framer-motion";
import {
  isElectron,
  getAudioDevices,
  setAudioDevice,
  startListening,
  cancelListening,
  getAudioLevel,
  listLocalModels,
  getTranscriptionSettings,
  setTranscriptionModel,
  getTranscriptionRuntimeStatus,
  downloadLocalModel,
  getCommandTemplates,
  saveCustomCommand,
  type AudioDevice,
  type CustomCommand,
  type LocalModelInfo,
  type TranscriptionRuntimeStatus,
} from "@/lib/desktop";

interface OnboardingModalProps {
  isOpen: boolean;
  onComplete: () => void;
}

type Step = "welcome" | "model" | "microphone" | "test" | "commands" | "complete";

function currentModelId(settings: { model: string }): string {
  return settings.model;
}

function isHandsFreeDeviceName(name: string): boolean {
  const normalized = name.toLowerCase();
  return normalized.includes("hands-free")
    || normalized.includes("hands free")
    || normalized.includes("ag audio")
    || normalized.includes("hfp")
    || normalized.includes("hsp");
}

export function OnboardingModal({ isOpen, onComplete }: OnboardingModalProps) {
  const [step, setStep] = useState<Step>("welcome");
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [selectedDevice, setSelectedDevice] = useState<string>("");
  const [deviceError, setDeviceError] = useState<string | null>(null);
  const [templates, setTemplates] = useState<CustomCommand[]>([]);
  const [selectedTemplates, setSelectedTemplates] = useState<Set<string>>(new Set());
  const [isTestingMic, setIsTestingMic] = useState(false);
  const [testSuccess, setTestSuccess] = useState(false);
  const [micTestError, setMicTestError] = useState<string | null>(null);
  const [models, setModels] = useState<LocalModelInfo[]>([]);
  const [selectedModel, setSelectedModel] = useState("");
  const [runtimeStatus, setRuntimeStatus] = useState<TranscriptionRuntimeStatus | null>(null);
  const [modelSetupBusy, setModelSetupBusy] = useState(false);
  const [modelError, setModelError] = useState<string | null>(null);

  const loadData = useCallback(async () => {
    try {
      const [localModels, transcriptionSettings, status] = await Promise.all([
        listLocalModels(),
        getTranscriptionSettings(),
        getTranscriptionRuntimeStatus(),
      ]);
      setModels(localModels);
      setRuntimeStatus(status);
      const configuredModel = currentModelId(transcriptionSettings);
      const defaultModel = localModels.find((model) => model.id === configuredModel)
        ?? localModels.find((model) => model.selected)
        ?? localModels.find((model) => model.downloaded)
        ?? localModels[0];
      setSelectedModel(defaultModel?.id ?? configuredModel);
      setModelError(null);
    } catch (error) {
      console.error("Failed to load local model setup:", error);
      setModelError(error instanceof Error ? error.message : "Failed to load local model setup.");
    }

    try {
      const devs = await getAudioDevices();
      setDevices(devs);
      setDeviceError(null);

      // Select a safe default device (avoid hands-free profiles that can hijack output).
      const defaultDevice = devs.find((d) => d.is_default && !isHandsFreeDeviceName(d.name))
        ?? devs.find((d) => !isHandsFreeDeviceName(d.name));
      if (defaultDevice) {
        setSelectedDevice(defaultDevice.name);
      } else {
        setSelectedDevice("");
        setDeviceError("No supported microphone is available. Check microphone permission or connect another input, then retry.");
      }
    } catch (error) {
      console.error("Failed to load onboarding device data:", error);
      setDevices([]);
      setSelectedDevice("");
      setDeviceError(error instanceof Error
        ? error.message
        : "Could not access microphones. Check microphone permission and retry.");
    }

    try {
      setTemplates(await getCommandTemplates());
    } catch (error) {
      console.error("Failed to load onboarding command templates:", error);
      setTemplates([]);
    }
  }, []);

  useEffect(() => {
    if (isOpen && isElectron()) {
      loadData();
    }
  }, [isOpen, loadData]);

  const handleDeviceSelect = async (deviceName: string) => {
    setDeviceError(null);
    try {
      await setAudioDevice(deviceName);
      setSelectedDevice(deviceName);
    } catch (error) {
      console.error("Failed to set device:", error);
      setDeviceError("This microphone is not supported. Choose a non-hands-free input.");
    }
  };

  const handleMicrophoneContinue = async () => {
    if (!selectedDevice) {
      return;
    }
    setDeviceError(null);
    try {
      await setAudioDevice(selectedDevice);
      nextStep();
    } catch (error) {
      console.error("Failed to confirm selected device:", error);
      setDeviceError("Failed to save microphone. Choose another input and try again.");
    }
  };

  const handleModelContinue = async () => {
    if (!selectedModel) {
      setModelError("Choose a local transcription model.");
      return;
    }

    setModelSetupBusy(true);
    setModelError(null);
    try {
      const selected = models.find((model) => model.id === selectedModel);
      if (selected && !selected.downloaded) {
        await downloadLocalModel(selectedModel);
      }
      await setTranscriptionModel(selectedModel);
      const [updatedModels, status] = await Promise.all([
        listLocalModels(),
        getTranscriptionRuntimeStatus(),
      ]);
      setModels(updatedModels);
      setRuntimeStatus(status);
      nextStep();
    } catch (error) {
      console.error("Failed to set up local transcription model:", error);
      setModelError(error instanceof Error ? error.message : "Failed to set up the local model.");
    } finally {
      setModelSetupBusy(false);
    }
  };

  const handleTestMic = async () => {
    setIsTestingMic(true);
    setTestSuccess(false);
    setMicTestError(null);

    let captureStarted = false;
    try {
      if (selectedDevice) {
        await setAudioDevice(selectedDevice);
      }
      await startListening();
      captureStarted = true;

      let peak = 0;
      for (let attempt = 0; attempt < 18; attempt += 1) {
        await new Promise((resolve) => setTimeout(resolve, 80));
        peak = Math.max(peak, await getAudioLevel());
      }

      if (peak < 0.015) {
        setMicTestError("No microphone signal was detected. Speak while the test is running, check the selected input, or skip this test.");
      } else {
        setTestSuccess(true);
      }
    } catch (error) {
      setMicTestError(error instanceof Error ? error.message : "Microphone test failed.");
    } finally {
      if (captureStarted) {
        await cancelListening().catch(() => undefined);
      }
      setIsTestingMic(false);
    }
  };

  const handleTemplateToggle = (id: string) => {
    const newSelected = new Set(selectedTemplates);
    if (newSelected.has(id)) {
      newSelected.delete(id);
    } else {
      newSelected.add(id);
    }
    setSelectedTemplates(newSelected);
  };

  const handleComplete = async () => {
    // Save selected templates
    for (const templateId of selectedTemplates) {
      const template = templates.find((t) => t.id === templateId);
      if (template) {
        const command: CustomCommand = {
          ...template,
          id: crypto.randomUUID(),
          enabled: true,
          created_at: new Date().toISOString(),
          last_used: null,
          use_count: 0,
        };
        try {
          await saveCustomCommand(command);
        } catch (error) {
          console.error("Failed to save command:", error);
        }
      }
    }
    onComplete();
  };

  const steps: Step[] = ["welcome", "model", "microphone", "test", "commands", "complete"];
  const currentStepIndex = steps.indexOf(step);

  const nextStep = () => {
    const nextIndex = currentStepIndex + 1;
    if (nextIndex < steps.length) {
      setStep(steps[nextIndex]);
    }
  };

  const prevStep = () => {
    const prevIndex = currentStepIndex - 1;
    if (prevIndex >= 0) {
      setStep(steps[prevIndex]);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-[200] flex items-center justify-center bg-overlay">
      <motion.div
        initial={{ opacity: 0, scale: 0.98 }}
        animate={{ opacity: 1, scale: 1 }}
        transition={{ duration: 0.2 }}
        className="w-full max-w-md rounded-df bg-card p-6 shadow-xl"
      >
        {/* Progress */}
        <div className="mb-6 flex gap-1.5">
          {steps.map((s, i) => (
            <div
              key={s}
              className={`h-1 flex-1 rounded-lg transition-colors ${
                i <= currentStepIndex ? "bg-primary" : "bg-muted-border"
              }`}
            />
          ))}
        </div>

        <AnimatePresence mode="wait">
          {step === "welcome" && (
            <motion.div
              key="welcome"
              initial={{ opacity: 0, x: 10 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -10 }}
              transition={{ duration: 0.15 }}
              className="text-center"
            >
              <div className="mb-4 flex justify-center">
                <div className="flex h-14 w-14 items-center justify-center rounded-df bg-primary/10">
                  <img
                    src="/logo.svg"
                    alt="ListenOS Logo"
                    width={32}
                    height={32}
                    className="h-8 w-8"
                  />
                </div>
              </div>
              <h2 className="mb-1.5 text-xl font-normal text-foreground">Welcome to ListenOS</h2>
              <p className="mb-6 text-sm text-muted-foreground">
                Let&apos;s set up a few things to get you started.
              </p>
              <button
                onClick={nextStep}
                className="ui-button ui-button-primary w-full rounded-df bg-primary px-4 py-2.5 text-sm font-normal text-primary-foreground hover:bg-primary-hover"
              >
                Get Started
              </button>
            </motion.div>
          )}

          {step === "microphone" && (
            <motion.div
              key="microphone"
              initial={{ opacity: 0, x: 10 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -10 }}
              transition={{ duration: 0.15 }}
            >
              <h2 className="mb-1.5 text-lg font-normal text-foreground">Select Your Microphone</h2>
              <p className="mb-4 text-sm text-muted-foreground">
                Choose a microphone for voice commands. Hands-free Bluetooth inputs are blocked to prevent audio hijacking.
              </p>
              
              <div className="mb-4 space-y-2 max-h-48 overflow-y-auto">
                {devices.map((device) => {
                  const isHandsFree = isHandsFreeDeviceName(device.name);
                  return (
                  <button
                    key={device.name}
                    onClick={() => handleDeviceSelect(device.name)}
                    disabled={isHandsFree}
                    className={`ui-button flex w-full items-center justify-start gap-3 rounded-df border p-3 text-left transition-colors ${
                      selectedDevice === device.name
                        ? "border-primary bg-primary/10"
                        : isHandsFree
                        ? "border-muted-border bg-muted cursor-not-allowed"
                        : "border-muted-border bg-muted hover:bg-accent"
                    }`}
                  >
                    <div className={`flex h-8 w-8 items-center justify-center rounded-df ${
                      selectedDevice === device.name ? "bg-primary/20 text-primary" : "bg-muted text-muted-foreground"
                    }`}>
                      <svg className="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 11a7 7 0 01-7 7m0 0a7 7 0 01-7-7m7 7v4m0 0H8m4 0h4m-4-8a3 3 0 01-3-3V5a3 3 0 116 0v6a3 3 0 01-3 3z" />
                      </svg>
                    </div>
                    <div className="flex-1 min-w-0">
                      <p className="font-normal text-sm text-foreground truncate">{device.name}</p>
                      {isHandsFree ? (
                        <p className="text-xs text-warning">Blocked: can hijack headphone output</p>
                      ) : device.is_default ? (
                        <p className="text-xs text-muted-foreground">System default</p>
                      ) : null}
                    </div>
                    {selectedDevice === device.name && (
                      <svg className="h-4 w-4 text-primary flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
                      </svg>
                    )}
                  </button>
                  );
                })}
              </div>
              {deviceError && (
                <div className="mb-3 flex items-center justify-between gap-3 rounded-df border border-negative/20 bg-negative/5 p-3">
                  <p className="text-xs text-negative">{deviceError}</p>
                  <button
                    type="button"
                    onClick={() => void loadData()}
                    className="ui-button ui-button-outline shrink-0 rounded-df border border-muted-border bg-muted px-2.5 py-1.5 text-xs text-foreground hover:bg-accent"
                  >
                    Retry
                  </button>
                </div>
              )}

              <div className="flex gap-2">
                <button
                  onClick={prevStep}
                  className="ui-button ui-button-outline flex-1 rounded-df border border-muted-border bg-muted px-4 py-2 text-sm font-normal text-foreground hover:bg-accent"
                >
                  Back
                </button>
                <button
                  onClick={() => void handleMicrophoneContinue()}
                  disabled={!selectedDevice}
                  className="ui-button ui-button-primary flex-1 rounded-df bg-primary px-4 py-2 text-sm font-normal text-primary-foreground hover:bg-primary-hover disabled:opacity-50"
                >
                  Continue
                </button>
              </div>
            </motion.div>
          )}

          {step === "model" && (
            <motion.div
              key="model"
              initial={{ opacity: 0, x: 10 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -10 }}
              transition={{ duration: 0.15 }}
            >
              <h2 className="mb-1.5 text-lg font-normal text-foreground">Set up local transcription</h2>
              <p className="mb-4 text-sm text-muted-foreground">
                Choose a speech model. ListenOS will keep dictation on this device and download the model if needed.
              </p>

              <div className="mb-4 max-h-52 space-y-2 overflow-y-auto">
                {models.map((model) => {
                  const selected = selectedModel === model.id;
                  return (
                    <button
                      key={model.id}
                      type="button"
                      onClick={() => setSelectedModel(model.id)}
                      disabled={modelSetupBusy}
                      className={`ui-button flex w-full items-start gap-3 rounded-df border p-3 text-left transition-colors ${
                        selected ? "border-primary bg-primary/10" : "border-muted-border bg-muted hover:bg-accent"
                      }`}
                    >
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <span className="text-sm font-normal text-foreground">{model.label || model.id}</span>
                          {model.selected && (
                            <span className="rounded-df bg-primary/10 px-1.5 py-0.5 text-[10px] text-primary">Current</span>
                          )}
                        </div>
                        <p className="mt-0.5 text-xs text-muted-foreground">
                          {[model.filename, model.downloaded ? "Downloaded" : "Download required"]
                            .filter(Boolean)
                            .join(" · ")}
                        </p>
                      </div>
                      <div className={`mt-0.5 flex h-4 w-4 flex-shrink-0 items-center justify-center rounded-df border ${
                        selected ? "border-primary bg-primary" : "border-border"
                      }`}>
                        {selected && (
                          <svg className="h-2.5 w-2.5 text-primary-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={3} d="M5 13l4 4L19 7" />
                          </svg>
                        )}
                      </div>
                    </button>
                  );
                })}
                {models.length === 0 && !modelError && (
                  <div className="rounded-df border border-muted-border bg-muted p-3 text-sm text-muted-foreground">
                    Loading local models...
                  </div>
                )}
                {runtimeStatus?.phase === "Ready" && (
                  <p className="text-xs text-positive">
                    Local transcription is ready with {runtimeStatus.model}.
                  </p>
                )}
                {runtimeStatus?.phase === "ModelMissing" && !modelError && (
                  <p className="text-xs text-muted-foreground">
                    {runtimeStatus.model} is selected and needs to be downloaded before dictation can start.
                  </p>
                )}
                {modelError && (
                  <div className="flex items-center justify-between gap-3 rounded-df border border-negative/20 bg-negative/5 p-3">
                    <p className="text-xs text-negative">{modelError}</p>
                    <button
                      type="button"
                      onClick={() => void loadData()}
                      disabled={modelSetupBusy}
                      className="ui-button ui-button-outline shrink-0 rounded-df border border-muted-border bg-muted px-2.5 py-1.5 text-xs text-foreground hover:bg-accent"
                    >
                      Retry
                    </button>
                  </div>
                )}
              </div>

              <div className="flex gap-2">
                <button
                  onClick={prevStep}
                  disabled={modelSetupBusy}
                  className="ui-button ui-button-outline flex-1 rounded-df border border-muted-border bg-muted px-4 py-2 text-sm font-normal text-foreground hover:bg-accent"
                >
                  Back
                </button>
                <button
                  onClick={() => void handleModelContinue()}
                  disabled={modelSetupBusy || !selectedModel}
                  className="ui-button ui-button-primary flex-1 rounded-df bg-primary px-4 py-2 text-sm font-normal text-primary-foreground hover:bg-primary-hover disabled:opacity-50"
                >
                  {modelSetupBusy
                    ? "Setting up..."
                    : models.find((model) => model.id === selectedModel)?.downloaded
                      ? "Continue"
                      : "Download & continue"}
                </button>
              </div>
            </motion.div>
          )}

          {step === "test" && (
            <motion.div
              key="test"
              initial={{ opacity: 0, x: 10 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -10 }}
              transition={{ duration: 0.15 }}
              className="text-center"
            >
              <h2 className="mb-1.5 text-lg font-normal text-foreground">Test Your Microphone</h2>
              <p className="mb-4 text-sm text-muted-foreground">
                Let&apos;s make sure your microphone is working.
              </p>

              <div className="mb-4 flex justify-center">
                {isTestingMic ? (
                  <div className="flex h-16 w-16 items-center justify-center rounded-lg bg-primary/20">
                    <div className="flex h-8 items-center gap-0.5">
                      {[...Array(4)].map((_, i) => (
                        <motion.div
                          key={i}
                          className="w-1 rounded-lg bg-primary"
                          animate={{ height: [6, 20, 6] }}
                          transition={{
                            duration: 0.5,
                            repeat: Infinity,
                            delay: i * 0.1,
                          }}
                        />
                      ))}
                    </div>
                  </div>
                ) : testSuccess ? (
                  <div className="flex h-16 w-16 items-center justify-center rounded-lg bg-positive/10">
                    <svg className="h-8 w-8 text-positive" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
                    </svg>
                  </div>
                ) : (
                  <button
                    onClick={handleTestMic}
                    className="ui-button ui-button-primary flex h-16 w-16 items-center justify-center rounded-lg bg-primary text-primary-foreground transition-colors hover:bg-primary-hover"
                  >
                    <svg className="h-7 w-7 text-primary-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 11a7 7 0 01-7 7m0 0a7 7 0 01-7-7m7 7v4m0 0H8m4 0h4m-4-8a3 3 0 01-3-3V5a3 3 0 116 0v6a3 3 0 01-3 3z" />
                    </svg>
                  </button>
                )}
              </div>

              <p className="mb-4 text-sm text-muted-foreground">
                {isTestingMic
                  ? "Listening..."
                  : testSuccess
                  ? "Microphone input detected."
                  : "Click to test your microphone."}
              </p>
              {micTestError && (
                <p className="mb-4 text-xs text-negative">{micTestError}</p>
              )}

              <div className="flex gap-2">
                <button
                  onClick={prevStep}
                  className="ui-button ui-button-outline flex-1 rounded-df border border-muted-border bg-muted px-4 py-2 text-sm font-normal text-foreground hover:bg-accent"
                >
                  Back
                </button>
                <button
                  onClick={nextStep}
                  className="ui-button ui-button-primary flex-1 rounded-df bg-primary px-4 py-2 text-sm font-normal text-primary-foreground hover:bg-primary-hover"
                >
                  {testSuccess ? "Continue" : "Skip"}
                </button>
              </div>
            </motion.div>
          )}

          {step === "commands" && (
            <motion.div
              key="commands"
              initial={{ opacity: 0, x: 10 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -10 }}
              transition={{ duration: 0.15 }}
            >
              <h2 className="mb-1.5 text-lg font-normal text-foreground">Quick Start Commands</h2>
              <p className="mb-4 text-sm text-muted-foreground">
                Select some command templates to get started.
              </p>

              <div className="mb-4 max-h-48 space-y-2 overflow-y-auto">
                {templates.map((template) => (
                  <button
                    key={template.id}
                    onClick={() => handleTemplateToggle(template.id)}
                    className={`flex w-full items-center gap-3 rounded-df border p-3 text-left transition-colors ${
                      selectedTemplates.has(template.id)
                        ? "border-primary bg-primary/10"
                        : "border-border hover:bg-accent"
                    }`}
                  >
                    <div className={`flex h-8 w-8 items-center justify-center rounded-df text-base ${
                      selectedTemplates.has(template.id) ? "bg-primary/20" : "bg-muted"
                    }`}>
                      {template.name.includes("Morning") ? "🌅" :
                       template.name.includes("Focus") ? "🎯" :
                       template.name.includes("Meeting") ? "📹" :
                       template.name.includes("End") ? "🌙" :
                       template.name.includes("Music") ? "🎵" : "⚡"}
                    </div>
                    <div className="flex-1 min-w-0">
                      <p className="font-normal text-sm text-foreground">{template.name}</p>
                      <p className="text-xs text-muted-foreground truncate">&quot;{template.trigger_phrase}&quot;</p>
                    </div>
                    <div className={`flex h-4 w-4 items-center justify-center rounded-df border flex-shrink-0 ${
                      selectedTemplates.has(template.id)
                        ? "border-primary bg-primary"
                        : "border-border"
                    }`}>
                      {selectedTemplates.has(template.id) && (
                        <svg className="h-2.5 w-2.5 text-primary-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={3} d="M5 13l4 4L19 7" />
                        </svg>
                      )}
                    </div>
                  </button>
                ))}
              </div>

              <div className="flex gap-2">
                <button
                  onClick={prevStep}
                  className="ui-button ui-button-outline flex-1 rounded-df border border-muted-border bg-muted px-4 py-2 text-sm font-normal text-foreground hover:bg-accent"
                >
                  Back
                </button>
                <button
                  onClick={nextStep}
                  className="ui-button ui-button-primary flex-1 rounded-df bg-primary px-4 py-2 text-sm font-normal text-primary-foreground hover:bg-primary-hover"
                >
                  Continue
                </button>
              </div>
            </motion.div>
          )}

          {step === "complete" && (
            <motion.div
              key="complete"
              initial={{ opacity: 0, x: 10 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -10 }}
              transition={{ duration: 0.15 }}
              className="text-center"
            >
              <div className="mb-4 flex justify-center">
                <motion.div
                  initial={{ scale: 0.8 }}
                  animate={{ scale: 1 }}
                  transition={{ type: "spring", duration: 0.3 }}
                  className="flex h-14 w-14 items-center justify-center rounded-df bg-positive/10"
                >
                  <svg className="h-7 w-7 text-positive" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
                  </svg>
                </motion.div>
              </div>
              <h2 className="mb-1.5 text-xl font-normal text-foreground">You&apos;re All Set!</h2>
              <p className="mb-4 text-sm text-muted-foreground">
                ListenOS is ready to use.
              </p>

              <div className="mb-6 rounded-df bg-muted p-3 text-left">
                <div className="flex items-center gap-2 mb-2">
                  <kbd className="rounded-df bg-card px-1.5 py-0.5 text-xs font-sans">Ctrl</kbd>
                  <span className="text-muted-foreground text-xs">+</span>
                  <kbd className="rounded-df bg-card px-1.5 py-0.5 text-xs font-sans">Space</kbd>
                  <span className="text-sm text-foreground">Hold to speak</span>
                </div>
                <p className="text-xs text-muted-foreground">
                  Release when done. ListenOS will process your command instantly.
                </p>
              </div>

              <button
                onClick={handleComplete}
                className="ui-button ui-button-primary w-full rounded-df bg-primary px-4 py-2.5 text-sm font-normal text-primary-foreground hover:bg-primary-hover"
              >
                Start Using ListenOS
              </button>
            </motion.div>
          )}
        </AnimatePresence>
      </motion.div>
    </div>
  );
}

