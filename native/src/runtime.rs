use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, Sender},
};
use std::time::Duration;

use crate::runtime_compute_status;
use origin_speak_lib::{
    AppState, DeliveryPhase, ShortcutEvent, ShortcutService, State, VoiceProcessingResult,
    get_audio_level, normalize_hotkey_string, process_captured_audio, start_listening,
    take_capture_for_processing,
};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::{mpsc as tokio_mpsc, watch};

const AUDIO_LEVEL_INTERVAL: Duration = Duration::from_millis(45);
const MODEL_PREWARM_DELAY: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RuntimeEvent {
    AudioLevel(f32),
    Idle,
    Listening,
    Processing,
    Success,
    Error,
}

#[derive(Debug)]
enum ControllerCommand {
    Shortcut(ShortcutEvent),
    ProcessingFinished(Result<VoiceProcessingResult, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureMode {
    Idle,
    Trigger,
    Processing,
}

fn shortcut_command(event: ShortcutEvent) -> ControllerCommand {
    ControllerCommand::Shortcut(event)
}

pub struct RuntimeController {
    _state: Arc<AppState>,
    events: Arc<Mutex<Receiver<RuntimeEvent>>>,
    commands: tokio_mpsc::UnboundedSender<ControllerCommand>,
    shortcuts: Option<ShortcutService>,
    trigger_hotkey: String,
    _runtime: Runtime,
}

impl RuntimeController {
    pub fn new() -> Result<Self, String> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .thread_name("origin-speak-native-runtime")
            .build()
            .map_err(|error| format!("Failed to create native runtime: {error}"))?;
        let state = Arc::new(AppState::default());
        let model_status = state.transcription.runtime_status();
        if !model_status.model_downloaded {
            return Err(format!(
                "Selected local transcription model '{}' is not installed or is invalid. Run `origin model download {}` before starting the resident runtime.",
                model_status.model, model_status.model
            ));
        }
        let (event_tx, event_rx) = mpsc::channel::<RuntimeEvent>();
        let events = Arc::new(Mutex::new(event_rx));
        let (command_tx, command_rx) = tokio_mpsc::unbounded_channel::<ControllerCommand>();
        let (audio_level_mode_tx, audio_level_mode_rx) = watch::channel(false);

        let trigger_hotkey = runtime.block_on(async {
            let mut config = state.config.lock().await;
            let normalized = normalize_hotkey_string(&config.trigger_hotkey)?;
            if normalized != config.trigger_hotkey {
                config.trigger_hotkey = normalized.clone();
                config.save_to_disk()?;
            }
            Ok::<String, String>(normalized)
        })?;

        runtime.spawn(run_command_loop(
            command_rx,
            command_tx.clone(),
            state.clone(),
            event_tx.clone(),
            audio_level_mode_tx,
        ));
        runtime.spawn(run_audio_level_loop(
            state.clone(),
            event_tx,
            audio_level_mode_rx,
        ));

        // Keep login/startup cheap: global shortcuts and the GPUI event loop must
        // become responsive before we begin large model I/O. The selected model
        // is warmed only after a short idle window; a dictation started sooner
        // can still load it on demand through the normal transcription path.
        let transcription = state.transcription.clone();
        runtime.spawn(async move {
            tokio::time::sleep(MODEL_PREWARM_DELAY).await;
            if transcription.runtime_status().model_downloaded {
                let result = transcription.preload_selected_model().await;
                let _ = runtime_compute_status::publish(&transcription.runtime_status());
                #[cfg(debug_assertions)]
                if let Err(error) = result {
                    eprintln!("[Origin Speak] model prewarm failed: {error}");
                }
                #[cfg(not(debug_assertions))]
                let _ = result;
            }
        });

        Ok(Self {
            _state: state,
            events,
            commands: command_tx,
            shortcuts: None,
            trigger_hotkey,
            _runtime: runtime,
        })
    }

    /// Register native global shortcuts on the GPUI/platform event-loop thread.
    pub fn initialize_shortcuts(&mut self) -> Result<(), String> {
        if self.shortcuts.is_some() {
            return Ok(());
        }

        let dispatch = self.commands.clone();
        let service = ShortcutService::new(&self.trigger_hotkey, move |event| {
            let _ = dispatch.send(shortcut_command(event));
        })
        .map_err(|error| format!("Global dictation shortcut unavailable: {error}"))?;
        self.shortcuts = Some(service);
        Ok(())
    }

    pub fn events(&self) -> Arc<Mutex<Receiver<RuntimeEvent>>> {
        self.events.clone()
    }
}

async fn run_command_loop(
    mut commands: tokio_mpsc::UnboundedReceiver<ControllerCommand>,
    command_tx: tokio_mpsc::UnboundedSender<ControllerCommand>,
    state: Arc<AppState>,
    events: Sender<RuntimeEvent>,
    audio_level_mode: watch::Sender<bool>,
) {
    let mut capture_mode = CaptureMode::Idle;

    while let Some(command) = commands.recv().await {
        match command {
            ControllerCommand::Shortcut(ShortcutEvent::TriggerPressed)
                if capture_mode == CaptureMode::Idle =>
            {
                // Do not expose Listening until CPAL has actually acquired and
                // started the input stream. This prevents a false listening
                // flash when the device is unavailable or permission is denied.
                match start_listening(State::new(state.as_ref())).await {
                    Ok(_) => {
                        capture_mode = CaptureMode::Trigger;
                        let _ = audio_level_mode.send(true);
                        let _ = events.send(RuntimeEvent::Listening);
                    }
                    Err(_) => {
                        let _ = audio_level_mode.send(false);
                        let _ = events.send(RuntimeEvent::Error);
                    }
                }
            }
            ControllerCommand::Shortcut(ShortcutEvent::TriggerReleased)
                if capture_mode == CaptureMode::Trigger =>
            {
                let captured = take_capture_for_processing(State::new(state.as_ref())).await;
                let _ = audio_level_mode.send(false);

                match captured {
                    Err(_) => {
                        capture_mode = CaptureMode::Idle;
                        let _ = events.send(RuntimeEvent::Error);
                    }
                    Ok(captured) => {
                        // Only one dictation may own the model at a time. Before
                        // this guard, repeated shortcut attempts spawned more
                        // blocking inference workers which queued on the model
                        // mutex and made shutdown wait for the entire backlog.
                        capture_mode = CaptureMode::Processing;
                        let _ = events.send(RuntimeEvent::Processing);
                        let task_state = state.clone();
                        let task_commands = command_tx.clone();
                        tokio::spawn(async move {
                            let result =
                                process_captured_audio(State::new(task_state.as_ref()), captured)
                                    .await;
                            let _ = runtime_compute_status::publish(
                                &task_state.transcription.runtime_status(),
                            );
                            let _ =
                                task_commands.send(ControllerCommand::ProcessingFinished(result));
                        });
                    }
                }
            }
            ControllerCommand::ProcessingFinished(result)
                if capture_mode == CaptureMode::Processing =>
            {
                capture_mode = CaptureMode::Idle;
                emit_processing_result(result, &events);
            }
            ControllerCommand::ProcessingFinished(_) => {}
            ControllerCommand::Shortcut(
                ShortcutEvent::TriggerPressed | ShortcutEvent::TriggerReleased,
            ) => {}
        }
    }
}

fn emit_processing_result(
    result: Result<VoiceProcessingResult, String>,
    events: &Sender<RuntimeEvent>,
) {
    match result {
        Ok(result) if result.transcription.text.trim().is_empty() => {
            let _ = events.send(RuntimeEvent::Idle);
        }
        Ok(result) if result.delivery_status.phase == DeliveryPhase::RecoverableFailure => {
            let _ = events.send(RuntimeEvent::Error);
        }
        Ok(result) if !result.delivered => {
            let _ = events.send(RuntimeEvent::Error);
        }
        Ok(_) => {
            let _ = events.send(RuntimeEvent::Success);
        }
        Err(_) => {
            let _ = events.send(RuntimeEvent::Error);
        }
    }
}

async fn run_audio_level_loop(
    state: Arc<AppState>,
    events: Sender<RuntimeEvent>,
    mut active: watch::Receiver<bool>,
) {
    let mut last_level = 0.0_f32;
    loop {
        if !*active.borrow() {
            if last_level != 0.0 {
                if events.send(RuntimeEvent::AudioLevel(0.0)).is_err() {
                    break;
                }
                last_level = 0.0;
            }
            if active.changed().await.is_err() {
                break;
            }
            continue;
        }

        tokio::select! {
            changed = active.changed() => {
                if changed.is_err() {
                    break;
                }
                continue;
            }
            _ = tokio::time::sleep(AUDIO_LEVEL_INTERVAL) => {}
        }

        if !*active.borrow() {
            continue;
        }
        let level = get_audio_level(State::new(state.as_ref()))
            .await
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        if events.send(RuntimeEvent::AudioLevel(level)).is_err() {
            break;
        }
        last_level = level;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictation_shortcut_keeps_press_and_release() {
        assert!(matches!(
            shortcut_command(ShortcutEvent::TriggerPressed),
            ControllerCommand::Shortcut(ShortcutEvent::TriggerPressed)
        ));
        assert!(matches!(
            shortcut_command(ShortcutEvent::TriggerReleased),
            ControllerCommand::Shortcut(ShortcutEvent::TriggerReleased)
        ));
    }

    #[test]
    fn processing_is_a_distinct_capture_mode() {
        assert_ne!(CaptureMode::Processing, CaptureMode::Idle);
        assert_ne!(CaptureMode::Processing, CaptureMode::Trigger);
    }
}
