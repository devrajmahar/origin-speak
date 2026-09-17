import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { motion, AnimatePresence } from "framer-motion";
import {
  isElectron,
  getStatus,
  startListening,
  cancelListening,
  stopListening,
  getPendingAction,
  confirmPendingAction,
  cancelPendingAction,
  onAssistantShortcut,
  onShortcutPressed,
  onShortcutReleased,
  getAudioLevel,
  listenDesktopEvent,
  PendingAction,
} from "@/lib/desktop";

type AssistantState = "idle" | "listening" | "handsfree" | "processing" | "success" | "error";
type NotificationType = "word-learned" | null;
const ONBOARDING_COMPLETE_KEY = "listenos_onboarding_complete";

function actionableBackendError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  const normalized = message.toLowerCase();

  if (normalized.includes("model") && (normalized.includes("missing") || normalized.includes("not found") || normalized.includes("download"))) {
    return `${message} Open ListenOS Settings → System to download or select a local transcription model.`;
  }

  return message || "ListenOS could not process that recording.";
}

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */
const PILL_SPRING = { type: "spring" as const, stiffness: 520, damping: 34 };
const CONTENT_FADE = { duration: 0.1 };

export default function AssistantPage() {
  const electronDesktop = isElectron();
  const [state, setState] = useState<AssistantState>("idle");
  const [rawAudioLevel, setRawAudioLevel] = useState(0);
  const [audioLevel, setAudioLevel] = useState(0);
  const [wavePhase, setWavePhase] = useState(0);
  const [notification, setNotification] = useState<NotificationType>(null);
  const [learnedWord, setLearnedWord] = useState<string | null>(null);
  const [pendingAction, setPendingAction] = useState<PendingAction | null>(null);
  const [statusNotice, setStatusNotice] = useState<string | null>(null);
  const stateRef = useRef<AssistantState>("idle");
  const rawAudioLevelRef = useRef(0);
  const wavePhaseRef = useRef(0);
  const audioLevelInterval = useRef<NodeJS.Timeout | null>(null);
  const statusInterval = useRef<NodeJS.Timeout | null>(null);
  const isStartingRef = useRef(false);
  const pendingStopRef = useRef(false);

  useEffect(() => { stateRef.current = state; }, [state]);
  useEffect(() => { rawAudioLevelRef.current = rawAudioLevel; }, [rawAudioLevel]);

  /* Simulate a changing microphone level so browser previews stay animated. */
  useEffect(() => {
    if (electronDesktop || (state !== "listening" && state !== "handsfree")) return;

    const startedAt = performance.now();
    const interval = window.setInterval(() => {
      const elapsed = (performance.now() - startedAt) / 1000;
      const primaryWave = (Math.sin(elapsed * 4.7) + 1) / 2;
      const detailWave = (Math.sin(elapsed * 11.3 + 0.8) + 1) / 2;
      setRawAudioLevel(0.12 + primaryWave * 0.55 + detailWave * 0.18);
    }, 45);

    return () => window.clearInterval(interval);
  }, [electronDesktop, state]);

  /* ---------------------------------------------------------------- */
  /*  Smooth audio level via rAF                                       */
  /* ---------------------------------------------------------------- */
  useEffect(() => {
    let rafId = 0;
    let lastTime = performance.now();
    const twoPi = Math.PI * 2;

    const tick = (time: number) => {
      const dt = Math.min(0.05, (time - lastTime) / 1000);
      lastTime = time;
      const active = stateRef.current === "listening" || stateRef.current === "handsfree";

      setAudioLevel((prev) => {
        const target = rawAudioLevelRef.current < 0.01 ? 0 : rawAudioLevelRef.current;
        const alpha = target > prev ? 1 - Math.exp(-dt * 30) : 1 - Math.exp(-dt * 10);
        let next = prev + (target - prev) * alpha;
        if (target === 0 && next < 0.008) next = 0;
        return Math.max(0, Math.min(1, next));
      });

      if (active) {
        wavePhaseRef.current = (wavePhaseRef.current + dt * 11) % twoPi;
        setWavePhase(wavePhaseRef.current);
      } else if (wavePhaseRef.current !== 0) {
        wavePhaseRef.current = 0;
        setWavePhase(0);
      }

      rafId = requestAnimationFrame(tick);
    };

    rafId = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(rafId);
  }, []);

  /* ---------------------------------------------------------------- */
  /*  Backend events                                                   */
  /* ---------------------------------------------------------------- */
  useEffect(() => {
    if (!electronDesktop) return;
    let unlisten: (() => void) | undefined;
    const setup = async () => {
      try {
        unlisten = await listenDesktopEvent<{ word: string }>("word-learned", (event) => {
          setLearnedWord(event.payload.word);
          setNotification("word-learned");
          setTimeout(() => setNotification(null), 3000);
        });
      } catch (e) { console.warn("Failed to listen for word-learned:", e); }
    };
    setup();
    return () => { unlisten?.(); };
  }, [electronDesktop]);

  useEffect(() => {
    if (!electronDesktop) return;
    getPendingAction().then(setPendingAction).catch(() => undefined);
  }, [electronDesktop]);

  /* ---------------------------------------------------------------- */
  /*  Audio level polling                                              */
  /* ---------------------------------------------------------------- */
  useEffect(() => {
    if ((state === "listening" || state === "handsfree") && isElectron()) {
      let stopped = false;
      const poll = async () => {
        if (stopped) return;
        try {
          const level = await getAudioLevel();
          if (stopped) return;
          const n = Math.max(0, Math.min(1, level));
          setRawAudioLevel(Math.max(0, (n - 0.018) / (1 - 0.018)));
        } catch { /* */ }
        if (!stopped) audioLevelInterval.current = setTimeout(poll, 45);
      };
      void poll();
      return () => { stopped = true; if (audioLevelInterval.current) clearTimeout(audioLevelInterval.current); };
    } else {
      if (audioLevelInterval.current) clearTimeout(audioLevelInterval.current);
      const resetTimer = window.setTimeout(() => setRawAudioLevel(0), 0);
      return () => window.clearTimeout(resetTimer);
    }
    return () => { if (audioLevelInterval.current) clearTimeout(audioLevelInterval.current); };
  }, [state]);

  useEffect(() => {
    if (!(state === "listening" || state === "handsfree" || state === "processing") || !isElectron()) {
      if (statusInterval.current) clearTimeout(statusInterval.current);
      return;
    }

    let stopped = false;
    const poll = async () => {
      if (stopped) return;
      try {
        const status = await getStatus();
        if (stopped) return;

        if (status.audio_status.phase === "Recovering") {
          setStatusNotice("Recovering microphone");
        } else if (stateRef.current === "processing") {
          const phase = status.delivery_status.phase;
          if (phase === "Retrying" || phase === "Verifying" || phase === "Injecting") {
            setStatusNotice(status.delivery_status.summary);
          } else if (phase !== "RecoverableFailure") {
            setStatusNotice(null);
          }
        } else if (statusNotice === "Recovering microphone") {
          setStatusNotice(null);
        }
      } catch {
        // Ignore transient polling errors while the backend is busy.
      }

      if (!stopped) {
        statusInterval.current = setTimeout(poll, 160);
      }
    };

    void poll();
    return () => {
      stopped = true;
      if (statusInterval.current) clearTimeout(statusInterval.current);
    };
  }, [state, statusNotice]);

  /* ---------------------------------------------------------------- */
  /*  Actions                                                          */
  /* ---------------------------------------------------------------- */
  const stopInternal = useCallback(async () => {
    if (stateRef.current !== "listening" && stateRef.current !== "handsfree") return;

    if (!isElectron()) {
      setState("processing");
      setTimeout(() => {
        setState("success");
        setTimeout(() => setState("idle"), 900);
      }, 900);
      return;
    }

    const dictationOnly = stateRef.current === "handsfree";
    setState("processing");
    try {
      const result = await stopListening(dictationOnly);
      const delivery = result.delivery_status;

      if (delivery.phase === "RecoverableFailure") {
        setStatusNotice(delivery.summary);
        setState("error");
        setTimeout(() => {
          setStatusNotice(null);
          setState("idle");
        }, 2600);
        return;
      }

      if (delivery.attempts > 1 || delivery.recovered_to_clipboard) {
        setStatusNotice(delivery.summary);
        setTimeout(() => setStatusNotice(null), 2400);
      }

      if (result.action?.action_type === "NoAction") { setState("idle"); return; }
      const isConvo = result.action?.action_type === "Respond" || result.action?.action_type === "Clarify";
      if (result.action?.requires_confirmation) {
        setPendingAction(await getPendingAction());
        setState("success");
        return;
      }
      if (!result.executed && result.action?.action_type !== "NoAction") {
        setState("error");
        setTimeout(() => setState("idle"), 1800);
        return;
      }
      setState("success");
      setTimeout(() => setState("idle"), isConvo ? 4500 : 1000);
    } catch (error) {
      setStatusNotice(actionableBackendError(error));
      setState("error");
      setTimeout(() => {
        setStatusNotice(null);
        setState("idle");
      }, 4200);
    }
  }, []);

  const start = useCallback(async (handsfree = false) => {
    if (stateRef.current !== "idle" || pendingAction || isStartingRef.current) return;

    if (isElectron()) {
      try {
        if (localStorage.getItem(ONBOARDING_COMPLETE_KEY) !== "true") {
          setStatusNotice("Finish ListenOS setup in the dashboard before starting dictation.");
          setState("error");
          setTimeout(() => {
            setStatusNotice(null);
            setState("idle");
          }, 3200);
          return;
        }
      } catch {
        // Backend readiness remains authoritative when browser storage is unavailable.
      }
    }

    isStartingRef.current = true;
    pendingStopRef.current = false;
    setState(handsfree ? "handsfree" : "listening");

    if (!isElectron()) {
      isStartingRef.current = false;
      return;
    }

    try {
      await startListening();
    } catch (error) {
      setStatusNotice(actionableBackendError(error));
      setState("error");
      setTimeout(() => {
        setStatusNotice(null);
        setState("idle");
      }, 4200);
    } finally {
      isStartingRef.current = false;
      if (pendingStopRef.current) { pendingStopRef.current = false; void stopInternal(); }
    }
  }, [pendingAction, stopInternal]);

  const stop = useCallback(async () => {
    if (isStartingRef.current) { pendingStopRef.current = true; return; }
    await stopInternal();
  }, [stopInternal]);

  const handleConfirmPending = useCallback(async () => {
    if (!pendingAction) return;
    setState("processing");
    try { await confirmPendingAction(); setPendingAction(null); setState("success"); setTimeout(() => setState("idle"), 900); }
    catch { setState("error"); setTimeout(() => setState("idle"), 1200); }
  }, [pendingAction]);

  const handleCancelPending = useCallback(async () => {
    if (!pendingAction) return;
    try { await cancelPendingAction(); setPendingAction(null); setState("success"); setTimeout(() => setState("idle"), 800); }
    catch { setState("error"); setTimeout(() => setState("idle"), 1200); }
  }, [pendingAction]);

  const cancel = useCallback(() => {
    if (isElectron()) {
      void cancelListening().catch((error) => {
        console.warn("Failed to cancel microphone capture:", error);
      });
    }
    setStatusNotice(null);
    setState("idle");
  }, []);

  const showBrowserPreview = useCallback((previewState: "listening" | "handsfree" | "processing" | "error") => {
    setStatusNotice(null);
    setPendingAction(null);
    setState(previewState);
  }, []);

  /* ---------------------------------------------------------------- */
  /*  Shortcuts                                                        */
  /* ---------------------------------------------------------------- */
  useEffect(() => {
    if (!electronDesktop) return;
    let u1: (() => void) | undefined;
    let u2: (() => void) | undefined;
    let u3: (() => void) | undefined;
    const setup = async () => {
      try {
        u1 = await onShortcutPressed(() => { if (stateRef.current === "idle") start(false); });
        u2 = await onShortcutReleased(() => { if (stateRef.current === "listening") stop(); });
        u3 = await onAssistantShortcut(() => {
          if (stateRef.current === "idle") {
            start(true);
            return;
          }

          if (stateRef.current === "handsfree") {
            stop();
          }
        });
      } catch (e) { console.warn("Setup failed:", e); }
    };
    setup();
    return () => { u1?.(); u2?.(); u3?.(); };
  }, [electronDesktop, start, stop]);

  /* ---------------------------------------------------------------- */
  /*  Pill dimensions                                                  */
  /* ---------------------------------------------------------------- */
  const pillWidth = state === "handsfree" ? 148 : state === "listening" ? 110 : state === "processing" || state === "error" ? 64 : state === "success" ? 52 : 40;
  const pillHeight = state === "idle" ? 16 : 28;

  return (
    <div className="relative flex h-full w-full flex-col items-center justify-end py-[6px]" style={{ background: "transparent" }}>
      {!electronDesktop && (
        <div
          className="keep-bg absolute left-1/2 top-6 z-50 w-[min(440px,calc(100vw-32px))] -translate-x-1/2 rounded-xl border border-border p-4 shadow-2xl"
          style={{ background: "var(--card)" }}
        >
          <div className="keep-bg mb-3 flex items-start justify-between gap-4">
            <div className="keep-bg">
              <p className="keep-bg text-sm text-foreground">Animation preview</p>
              <p className="keep-bg mt-1 text-xs text-muted-foreground">Test the real overlay states without the desktop microphone.</p>
            </div>
            <span className="keep-bg rounded-df border border-border bg-muted px-2 py-1 text-[9px] uppercase tracking-[0.12em] text-muted-foreground">
              Browser only
            </span>
          </div>
          <div className="keep-bg grid grid-cols-2 gap-2">
            {([
              { state: "listening", label: "Listening", detail: "Reactive wave" },
              { state: "handsfree", label: "Hands-free", detail: "Wave + controls" },
              { state: "processing", label: "Processing", detail: "Loading dashes" },
              { state: "error", label: "Error", detail: "Failure shake" },
            ] as const).map((preview) => (
              <button
                key={preview.state}
                type="button"
                aria-pressed={state === preview.state}
                onClick={() => showBrowserPreview(preview.state)}
                className="keep-bg rounded-lg border border-muted-border bg-muted px-3 py-2.5 text-left transition-colors hover:border-primary/40 hover:bg-accent aria-pressed:border-primary/60 aria-pressed:bg-primary/10"
              >
                <span className="keep-bg block text-xs text-foreground">{preview.label}</span>
                <span className="keep-bg mt-0.5 block text-[10px] text-muted-foreground">{preview.detail}</span>
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Notification */}
      <AnimatePresence>
        {notification && learnedWord && (
          <motion.div
            initial={{ opacity: 0, y: 10, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 6, scale: 0.96 }}
            className="absolute bottom-full mb-3 h-8 px-3 rounded-lg keep-bg flex items-center gap-2"
            style={{ background: "var(--muted)", border: "1px solid var(--muted-border)" }}
          >
            <div className="w-2 h-2 rounded-lg bg-positive keep-bg" />
            <span className="text-[10px] text-muted-foreground truncate max-w-36">{learnedWord}</span>
          </motion.div>
        )}
      </AnimatePresence>

      <AnimatePresence>
        {statusNotice && !pendingAction && (
          <motion.div
            initial={{ opacity: 0, y: 10, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 6, scale: 0.96 }}
            className="absolute bottom-full mb-3 max-w-[320px] px-3 py-2 rounded-df keep-bg"
            style={{ background: "var(--muted)", border: "1px solid var(--muted-border)" }}
          >
            <span className="text-[11px] text-muted-foreground">{statusNotice}</span>
          </motion.div>
        )}
      </AnimatePresence>

      {/* Pending confirmation */}
      <AnimatePresence>
        {pendingAction && (
          <motion.div
            initial={{ opacity: 0, y: 14, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 10, scale: 0.96 }}
            className="absolute bottom-full mb-3 px-4 py-3 rounded-df keep-bg w-[320px]"
            style={{ background: "var(--muted)", border: "1px solid var(--muted-border)" }}
          >
            <div className="flex flex-col gap-2">
              <span className="text-[10px] tracking-wide uppercase text-warning">Confirmation required</span>
              <span className="text-sm text-foreground">{pendingAction.summary}</span>
              <div className="flex items-center gap-2 mt-1">
                <button onClick={handleConfirmPending} className="ui-button px-3 py-1.5 rounded-df bg-positive/10 hover:bg-positive/20 text-positive text-xs font-normal transition-colors">Confirm</button>
                <button onClick={handleCancelPending} className="ui-button px-3 py-1.5 rounded-df bg-negative/10 hover:bg-negative/20 text-negative text-xs font-normal transition-colors">Cancel</button>
              </div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      {/* ==================== PILL ==================== */}
      <motion.div
        className="relative rounded-lg cursor-pointer keep-bg"
        style={{ willChange: "transform, width, height" }}
        initial={false}
        animate={{ width: pillWidth, height: pillHeight }}
        transition={PILL_SPRING}
        onClick={() => { if (state === "idle") start(true); }}
      >
        {/* Every state keeps the same neutral border. */}
        <div
          className="absolute rounded-lg keep-bg overflow-hidden"
          style={{
            inset: 0,
            background: "var(--muted)",
            border: "1px solid color-mix(in oklab, var(--muted-foreground) 38%, var(--muted-border))",
          }}
        />

        {/* Content */}
        <div className="relative z-10 flex h-full w-full items-center justify-center overflow-hidden rounded-lg">
          <AnimatePresence initial={false}>
            {state === "listening" && <ListeningWave key="listening" level={audioLevel} phase={wavePhase} />}
            {state === "handsfree" && <HandsfreePill key="handsfree" level={audioLevel} phase={wavePhase} onCancel={cancel} onStop={stop} />}
            {state === "processing" && <ProcessingPill key="processing" />}
            {state === "success" && <SuccessPill key="success" />}
            {state === "error" && <ErrorPill key="error" />}
          </AnimatePresence>
        </div>
      </motion.div>
    </div>
  );
}

/* ================================================================== */
/*  LISTENING — fluid SVG waveform                                     */
/* ================================================================== */
function ListeningWave({ level, phase }: { level: number; phase: number }) {
  const barCount = 12;
  const energy = Math.pow(Math.max(0, Math.min(1, level)), 0.8);

  const heights = useMemo(() => {
    const h: number[] = [];
    for (let i = 0; i < barCount; i++) {
      const center = (barCount - 1) / 2;
      const dist = Math.abs(i - center) / center;
      const bellCurve = Math.exp(-dist * dist * 2.2);
      const wave1 = 0.7 + 0.3 * Math.sin(phase + i * 0.6);
      const wave2 = 0.85 + 0.15 * Math.sin(phase * 1.7 + i * 1.1);
      const minH = 2;
      const maxH = 20;
      h.push(minH + (maxH - minH) * energy * bellCurve * wave1 * wave2);
    }
    return h;
  }, [energy, phase]);

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={CONTENT_FADE}
      className="absolute inset-0 flex items-center justify-center gap-[1.5px] px-3"
    >
      {heights.map((h, i) => {
        const intensity = h / 20;
        return (
          <div
            key={i}
            className="rounded-lg keep-bg"
            style={{
              width: "2px",
              height: `${h.toFixed(1)}px`,
              background: "var(--primary)",
              opacity: 0.5 + intensity * 0.5,
              transition: "height 0.035s linear, background 0.08s linear",
              boxShadow: energy > 0.3 ? `0 0 ${Math.round(intensity * 6)}px color-mix(in oklab, var(--primary) 40%, transparent)` : "none",
            }}
          />
        );
      })}
    </motion.div>
  );
}

/* ================================================================== */
/*  HANDSFREE — waveform + controls                                    */
/* ================================================================== */
function HandsfreePill({ level, phase, onCancel, onStop }: { level: number; phase: number; onCancel: () => void; onStop: () => void }) {
  const barCount = 16;
  const energy = Math.pow(Math.max(0, Math.min(1, level)), 0.8);

  const heights = useMemo(() => {
    const h: number[] = [];
    for (let i = 0; i < barCount; i++) {
      const center = (barCount - 1) / 2;
      const dist = Math.abs(i - center) / center;
      const bell = Math.exp(-dist * dist * 2.2);
      const w1 = 0.7 + 0.3 * Math.sin(phase + i * 0.6);
      const w2 = 0.85 + 0.15 * Math.sin(phase * 1.7 + i * 1.1);
      h.push(3 + 19 * energy * bell * w1 * w2);
    }
    return h;
  }, [energy, phase]);

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={CONTENT_FADE}
      className="absolute inset-0 flex items-center justify-between px-1.5"
    >
      <button
        onClick={(e) => { e.stopPropagation(); onCancel(); }}
        aria-label="Cancel hands-free listening"
        className="w-5 h-5 rounded-lg bg-accent hover:bg-muted flex items-center justify-center transition-colors keep-bg flex-shrink-0"
      >
        <svg className="w-2.5 h-2.5 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth={2.5}>
          <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
        </svg>
      </button>
      <div className="mx-1 flex h-6 flex-1 items-center justify-center gap-[1.5px]">
        {heights.map((h, i) => {
          const intensity = h / 22;
          return (
            <div
              key={i}
              className="rounded-lg keep-bg"
              style={{
                width: "2px",
                height: `${h.toFixed(1)}px`,
                background: "var(--primary)",
                opacity: 0.5 + intensity * 0.5,
                transition: "height 0.035s linear",
              }}
            />
          );
        })}
      </div>
      <button
        onClick={(e) => { e.stopPropagation(); onStop(); }}
        aria-label="Stop and process hands-free listening"
        className="w-5 h-5 rounded-lg flex items-center justify-center transition-colors keep-bg flex-shrink-0"
        style={{ background: "var(--negative)" }}
      >
        <div className="w-[7px] h-[7px] rounded-sm bg-primary-foreground keep-bg" />
      </button>
    </motion.div>
  );
}

/* ================================================================== */
/*  PROCESSING — centered spinner                                     */
/* ================================================================== */
function ProcessingPill() {
  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={CONTENT_FADE}
      className="absolute inset-0 grid place-items-center"
    >
      <motion.div
        className="keep-bg h-3.5 w-3.5 rounded-full border-2 border-primary/25 border-t-primary"
        animate={{ rotate: 360 }}
        transition={{ duration: 0.72, repeat: Infinity, ease: "linear" }}
      />
    </motion.div>
  );
}

/* ================================================================== */
/*  SUCCESS — checkmark with spring pop                                */
/* ================================================================== */
function SuccessPill() {
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.3 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.8 }}
      transition={{ type: "spring", stiffness: 500, damping: 25 }}
      className="absolute inset-0 flex items-center justify-center"
    >
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" strokeLinecap="round" strokeLinejoin="round">
        <motion.path
          d="M5 13l4 4L19 7"
          stroke="var(--positive)"
          strokeWidth="2.5"
          initial={{ pathLength: 0 }}
          animate={{ pathLength: 1 }}
          transition={{ duration: 0.25, ease: "easeOut", delay: 0.05 }}
          style={{ filter: "drop-shadow(0 0 4px color-mix(in oklab, var(--positive) 60%, transparent))" }}
        />
      </svg>
    </motion.div>
  );
}

/* ================================================================== */
/*  ERROR — centered X                                                 */
/* ================================================================== */
function ErrorPill() {
  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={CONTENT_FADE}
      className="absolute inset-0 flex items-center justify-center"
    >
      <svg
        width="13" height="13" viewBox="0 0 24 24" fill="none" strokeLinecap="round" strokeLinejoin="round"
        style={{ filter: "drop-shadow(0 0 4px color-mix(in oklab, var(--negative) 60%, transparent))" }}
      >
        <path d="M18 6L6 18" stroke="var(--negative)" strokeWidth="2.5" />
        <path d="M6 6l12 12" stroke="var(--negative)" strokeWidth="2.5" />
      </svg>
    </motion.div>
  );
}
