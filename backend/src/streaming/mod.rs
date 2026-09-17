//! Real-time audio streaming module
//!
//! Captures microphone audio and accumulates it for processing.
//! Cross-platform support for Windows, macOS, and Linux.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AudioHealthPhase {
    Idle,
    Starting,
    Healthy,
    Recovering,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioRuntimeStatus {
    pub phase: AudioHealthPhase,
    pub device_name: Option<String>,
    pub restart_count: u32,
    pub last_error: Option<String>,
    pub last_callback_age_ms: Option<u64>,
}

#[derive(Debug, Clone)]
struct RuntimeMetrics {
    phase: AudioHealthPhase,
    device_name: Option<String>,
    restart_count: u32,
    last_error: Option<String>,
    last_callback_ms: Option<u64>,
    phase_changed_ms: u64,
    sample_rate_hz: u32,
}

impl Default for RuntimeMetrics {
    fn default() -> Self {
        Self {
            phase: AudioHealthPhase::Idle,
            device_name: None,
            restart_count: 0,
            last_error: None,
            last_callback_ms: None,
            phase_changed_ms: now_millis(),
            sample_rate_hz: SAMPLE_RATE,
        }
    }
}

/// Audio chunk for streaming (100ms of audio at 16kHz = 1600 samples)
#[allow(dead_code)]
pub const CHUNK_SIZE_MS: u32 = 100;
pub const SAMPLE_RATE: u32 = 16000;
#[allow(dead_code)]
pub const CHUNK_SAMPLES: usize = (SAMPLE_RATE * CHUNK_SIZE_MS / 1000) as usize;

/// Audio streaming state - thread-safe implementation
pub struct AudioStreamer {
    is_recording: Arc<AtomicBool>,
    accumulated_samples: Arc<Mutex<Vec<f32>>>,
    live_level_bits: Arc<AtomicU32>,
    last_stream_release_ms: Arc<AtomicU64>,
    lifecycle: Mutex<()>,
    capture_owner: Mutex<Option<CaptureOwner>>,
    runtime: Arc<Mutex<RuntimeMetrics>>,
}

/// Control plane for the thread that owns the non-Send CPAL stream.
///
/// The stream itself never leaves that thread; only this channel and join
/// handle are shared with `AudioStreamer`.
struct CaptureOwner {
    stop_tx: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl CaptureOwner {
    fn shutdown(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                log::error!("Audio capture owner thread panicked during shutdown");
            }
        }
    }
}

impl Drop for CaptureOwner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl AudioStreamer {
    pub fn new() -> Self {
        Self {
            is_recording: Arc::new(AtomicBool::new(false)),
            accumulated_samples: Arc::new(Mutex::new(Vec::with_capacity(
                SAMPLE_RATE as usize * 30,
            ))),
            live_level_bits: Arc::new(AtomicU32::new(0.0_f32.to_bits())),
            last_stream_release_ms: Arc::new(AtomicU64::new(0)),
            lifecycle: Mutex::new(()),
            capture_owner: Mutex::new(None),
            runtime: Arc::new(Mutex::new(RuntimeMetrics::default())),
        }
    }

    fn is_handsfree_device_name(name: &str) -> bool {
        let n = name.to_lowercase();
        n.contains("hands-free")
            || n.contains("hands free")
            || n.contains("ag audio")
            || n.contains("hfp")
            || n.contains("hsp")
    }

    fn get_device_name(device: &cpal::Device) -> String {
        device.name().unwrap_or_else(|_| "Unknown".to_string())
    }

    fn find_input_device_by_name(
        host: &cpal::Host,
        preferred_name: &str,
    ) -> Result<Option<cpal::Device>, String> {
        let wanted = preferred_name.trim().to_lowercase();
        if wanted.is_empty() {
            return Ok(None);
        }

        let mut match_device = None;
        let devices = host
            .input_devices()
            .map_err(|e| format!("Failed to enumerate input devices: {}", e))?;
        for device in devices {
            let name = Self::get_device_name(&device).to_lowercase();
            if name == wanted {
                match_device = Some(device);
                break;
            }
        }

        Ok(match_device)
    }

    fn first_non_handsfree_input_device(host: &cpal::Host) -> Result<Option<cpal::Device>, String> {
        let devices = host
            .input_devices()
            .map_err(|e| format!("Failed to enumerate input devices: {}", e))?;
        for device in devices {
            let candidate_name = Self::get_device_name(&device);
            if !Self::is_handsfree_device_name(&candidate_name) {
                return Ok(Some(device));
            }
        }
        Ok(None)
    }

    fn select_input_device(
        host: &cpal::Host,
        preferred_device_name: Option<&str>,
    ) -> Result<cpal::Device, String> {
        if let Some(preferred) = preferred_device_name {
            if let Some(device) = Self::find_input_device_by_name(host, preferred)? {
                let preferred_name = Self::get_device_name(&device);
                if Self::is_handsfree_device_name(&preferred_name) {
                    log::warn!(
                        "Preferred input '{}' is Bluetooth hands-free and can hijack headphone output. Ignoring it.",
                        preferred_name
                    );
                } else {
                    log::info!("Using preferred input device: {}", preferred_name);
                    return Ok(device);
                }
            }
            log::warn!(
                "Preferred input device '{}' not found, falling back to automatic selection",
                preferred
            );
        }

        let default_device = host.default_input_device();
        let default_name = default_device.as_ref().map(Self::get_device_name);

        if let Some(default) = default_device {
            if let Some(name) = default_name.as_ref() {
                // On many Bluetooth headsets, opening the Hands-Free input can steal output audio route.
                // Always prefer a non-handsfree input when available.
                if Self::is_handsfree_device_name(name) {
                    if let Some(device) = Self::first_non_handsfree_input_device(host)? {
                        let candidate_name = Self::get_device_name(&device);
                        log::warn!(
                            "Default input '{}' looks like Bluetooth hands-free. Using '{}' to avoid output audio hijack.",
                            name,
                            candidate_name
                        );
                        return Ok(device);
                    }
                }
            }
            return Ok(default);
        }

        // Last-resort fallback.
        if let Some(device) = Self::first_non_handsfree_input_device(host)? {
            return Ok(device);
        }

        let mut devices = host
            .input_devices()
            .map_err(|e| format!("Failed to enumerate input devices: {}", e))?;
        devices.next().ok_or_else(|| {
            "No input device available. Please check microphone permissions.".to_string()
        })
    }

    /// Start recording audio directly to internal buffer.
    ///
    /// CPAL stream creation, lifetime, and destruction stay on one dedicated
    /// owner thread. This avoids making `cpal::Stream` artificially Send/Sync
    /// while keeping this synchronous API's startup errors deterministic.
    pub fn start_streaming(&self, preferred_device_name: Option<&str>) -> Result<(), String> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.is_recording.load(Ordering::SeqCst) {
            return Err("Already recording".to_string());
        }

        if let Ok(mut samples) = self.accumulated_samples.lock() {
            samples.clear();
        }
        self.live_level_bits
            .store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.update_runtime(AudioHealthPhase::Starting, None, None, false);
        self.is_recording.store(true, Ordering::SeqCst);

        let is_recording = self.is_recording.clone();
        let accumulated = self.accumulated_samples.clone();
        let live_level = self.live_level_bits.clone();
        let runtime = self.runtime.clone();
        let preferred_device_name = preferred_device_name.map(str::to_owned);
        let (started_tx, started_rx) = mpsc::sync_channel::<Result<(String, u32), String>>(1);
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        let owner_thread = thread::Builder::new()
            .name("listenos-audio-capture".to_string())
            .spawn(move || {
                let result = run_capture_owner(
                    preferred_device_name.as_deref(),
                    is_recording,
                    accumulated,
                    live_level,
                    runtime,
                    stop_rx,
                    &started_tx,
                );
                if let Err(error) = result {
                    let _ = started_tx.send(Err(error));
                }
            })
            .map_err(|error| {
                self.is_recording.store(false, Ordering::SeqCst);
                let message = format!("Failed to start audio capture owner thread: {error}");
                self.update_runtime(AudioHealthPhase::Error, None, Some(message.clone()), false);
                message
            })?;

        {
            let mut owner = self
                .capture_owner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *owner = Some(CaptureOwner {
                stop_tx: Some(stop_tx),
                thread: Some(owner_thread),
            });
        }

        match started_rx.recv() {
            Ok(Ok((device_name, sample_rate))) => {
                log::info!(
                    "Audio streaming started on '{}' at {} Hz",
                    device_name,
                    sample_rate
                );
                Ok(())
            }
            Ok(Err(error)) => {
                self.is_recording.store(false, Ordering::SeqCst);
                self.release_capture_owner();
                self.update_runtime(AudioHealthPhase::Error, None, Some(error.clone()), false);
                Err(error)
            }
            Err(error) => {
                self.is_recording.store(false, Ordering::SeqCst);
                self.release_capture_owner();
                let message = format!("Audio capture owner exited during startup: {error}");
                self.update_runtime(AudioHealthPhase::Error, None, Some(message.clone()), false);
                Err(message)
            }
        }
    }

    /// Stop capture and return whether a live stream owner actually had to be
    /// released. Callers that are about to restart can use this to avoid the
    /// Windows device-release grace period on the normal cold-start path.
    pub fn stop_streaming(&self) -> bool {
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.is_recording.store(false, Ordering::SeqCst);
        self.live_level_bits
            .store(0.0_f32.to_bits(), Ordering::Relaxed);

        // CaptureOwner::drop signals the owner and joins it. The CPAL stream is
        // therefore dropped on its creator thread before we timestamp release.
        let released_active_stream = self.release_capture_owner();
        if released_active_stream {
            self.last_stream_release_ms
                .store(now_millis(), Ordering::Release);
        }

        self.update_runtime(AudioHealthPhase::Idle, None, None, false);

        log::info!("Audio streaming stopped");
        released_active_stream
    }

    fn release_capture_owner(&self) -> bool {
        let active_owner = self
            .capture_owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let had_owner = active_owner.is_some();
        drop(active_owner);
        had_owner
    }

    /// Remaining device-release grace interval after the most recent real
    /// stream drop. This keeps rapid stop -> start cycles reliable on Windows
    /// without imposing a fixed delay on a cold start.
    pub fn release_grace_remaining(&self, grace: Duration) -> Duration {
        let released_at = self.last_stream_release_ms.load(Ordering::Acquire);
        if released_at == 0 {
            return Duration::ZERO;
        }
        let elapsed_ms = now_millis().saturating_sub(released_at);
        grace.saturating_sub(Duration::from_millis(elapsed_ms))
    }

    /// Check if currently streaming
    pub fn is_streaming(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    /// Get accumulated samples
    pub fn get_accumulated_samples(&self) -> Vec<f32> {
        self.accumulated_samples
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// Clear accumulated samples
    pub fn clear_samples(&self) {
        if let Ok(mut samples) = self.accumulated_samples.lock() {
            samples.clear();
        }
        self.live_level_bits
            .store(0.0_f32.to_bits(), Ordering::Relaxed);
    }

    /// Get current live audio level from the capture callback (0.0-1.0).
    pub fn get_live_level(&self) -> f32 {
        f32::from_bits(self.live_level_bits.load(Ordering::Relaxed)).clamp(0.0, 1.0)
    }

    pub fn snapshot_runtime_status(&self) -> AudioRuntimeStatus {
        let now = now_millis();
        self.runtime
            .lock()
            .map(|runtime| AudioRuntimeStatus {
                phase: runtime.phase.clone(),
                device_name: runtime.device_name.clone(),
                restart_count: runtime.restart_count,
                last_error: runtime.last_error.clone(),
                last_callback_age_ms: runtime
                    .last_callback_ms
                    .map(|timestamp| now.saturating_sub(timestamp)),
            })
            .unwrap_or(AudioRuntimeStatus {
                phase: AudioHealthPhase::Error,
                device_name: None,
                restart_count: 0,
                last_error: Some("Audio runtime state unavailable".to_string()),
                last_callback_age_ms: None,
            })
    }

    pub fn current_sample_rate(&self) -> u32 {
        self.runtime
            .lock()
            .map(|runtime| runtime.sample_rate_hz)
            .unwrap_or(SAMPLE_RATE)
    }

    pub fn should_restart(&self, healthy_stall_after: Duration, startup_timeout: Duration) -> bool {
        if !self.is_streaming() {
            return false;
        }

        let now = now_millis();
        self.runtime
            .lock()
            .map(|runtime| {
                let callback_age = runtime.last_callback_ms.map(|ts| now.saturating_sub(ts));
                let phase_age = now.saturating_sub(runtime.phase_changed_ms);
                match runtime.phase {
                    AudioHealthPhase::Healthy => callback_age
                        .map(|age| age > healthy_stall_after.as_millis() as u64)
                        .unwrap_or(phase_age > startup_timeout.as_millis() as u64),
                    AudioHealthPhase::Starting | AudioHealthPhase::Recovering => {
                        phase_age > startup_timeout.as_millis() as u64
                    }
                    AudioHealthPhase::Error => true,
                    AudioHealthPhase::Idle => false,
                }
            })
            .unwrap_or(false)
    }

    pub fn mark_recovering(&self, reason: impl Into<String>) {
        self.update_runtime(
            AudioHealthPhase::Recovering,
            None,
            Some(reason.into()),
            true,
        );
    }

    pub fn mark_error(&self, error: impl Into<String>) {
        self.update_runtime(AudioHealthPhase::Error, None, Some(error.into()), false);
    }

    fn update_runtime(
        &self,
        phase: AudioHealthPhase,
        device_name: Option<String>,
        last_error: Option<String>,
        increment_restart_count: bool,
    ) {
        if let Ok(mut runtime) = self.runtime.lock() {
            runtime.phase = phase;
            if let Some(device_name) = device_name {
                runtime.device_name = Some(device_name);
            }
            runtime.last_error = last_error;
            runtime.phase_changed_ms = now_millis();
            if increment_restart_count {
                runtime.restart_count = runtime.restart_count.saturating_add(1);
            }
            if matches!(runtime.phase, AudioHealthPhase::Idle) {
                runtime.last_callback_ms = None;
            }
        }
    }
}

fn run_capture_owner(
    preferred_device_name: Option<&str>,
    is_recording: Arc<AtomicBool>,
    accumulated: Arc<Mutex<Vec<f32>>>,
    live_level: Arc<AtomicU32>,
    runtime: Arc<Mutex<RuntimeMetrics>>,
    stop_rx: mpsc::Receiver<()>,
    started_tx: &mpsc::SyncSender<Result<(String, u32), String>>,
) -> Result<(), String> {
    let host = cpal::default_host();
    log::info!("Audio host: {}", host.id().name());

    let device = AudioStreamer::select_input_device(&host, preferred_device_name)?;
    let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
    log::info!("Using input device: {}", device_name);

    // Use the device's native shared-mode format first; this is less likely to
    // trigger routing changes or device lockups than forcing a custom format.
    let supported_config = device
        .default_input_config()
        .map_err(|error| format!("Failed to get default input config: {error}"))?;
    let sample_rate = supported_config.sample_rate().0;
    let sample_format = supported_config.sample_format();
    let config: cpal::StreamConfig = supported_config.into();
    let channels = config.channels;
    log::info!("Default input config: {:?}", config);

    if let Ok(mut metrics) = runtime.lock() {
        metrics.phase = AudioHealthPhase::Starting;
        metrics.device_name = Some(device_name.clone());
        metrics.last_error = None;
        metrics.phase_changed_ms = now_millis();
        metrics.sample_rate_hz = sample_rate;
    }

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let callback_recording = is_recording.clone();
            let callback_accumulated = accumulated.clone();
            let callback_level = live_level.clone();
            let callback_runtime = runtime.clone();
            let error_runtime = runtime.clone();
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    process_input_data(
                        data,
                        channels,
                        callback_recording.clone(),
                        callback_accumulated.clone(),
                        callback_level.clone(),
                        callback_runtime.clone(),
                    );
                },
                move |error| record_stream_error(&error_runtime, error.to_string()),
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let callback_recording = is_recording.clone();
            let callback_accumulated = accumulated.clone();
            let callback_level = live_level.clone();
            let callback_runtime = runtime.clone();
            let error_runtime = runtime.clone();
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let converted = data
                        .iter()
                        .map(|sample| *sample as f32 / i16::MAX as f32)
                        .collect::<Vec<_>>();
                    process_input_data(
                        &converted,
                        channels,
                        callback_recording.clone(),
                        callback_accumulated.clone(),
                        callback_level.clone(),
                        callback_runtime.clone(),
                    );
                },
                move |error| record_stream_error(&error_runtime, error.to_string()),
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let callback_recording = is_recording.clone();
            let callback_accumulated = accumulated.clone();
            let callback_level = live_level.clone();
            let callback_runtime = runtime.clone();
            let error_runtime = runtime.clone();
            device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    let converted = data
                        .iter()
                        .map(|sample| (*sample as f32 / u16::MAX as f32) * 2.0 - 1.0)
                        .collect::<Vec<_>>();
                    process_input_data(
                        &converted,
                        channels,
                        callback_recording.clone(),
                        callback_accumulated.clone(),
                        callback_level.clone(),
                        callback_runtime.clone(),
                    );
                },
                move |error| record_stream_error(&error_runtime, error.to_string()),
                None,
            )
        }
        _ => Err(cpal::BuildStreamError::StreamConfigNotSupported),
    }
    .map_err(|error| {
        format!("Failed to build audio stream: {error}. Check microphone permissions.")
    })?;

    stream
        .play()
        .map_err(|error| format!("Failed to start audio stream: {error}"))?;

    started_tx
        .send(Ok((device_name, sample_rate)))
        .map_err(|_| "Audio startup listener disconnected".to_string())?;

    // Keep the stream local to this thread. A stop signal or sender disconnect
    // ends the owner loop and drops the stream here, on the creator thread.
    let _ = stop_rx.recv();
    drop(stream);
    Ok(())
}

fn record_stream_error(runtime: &Arc<Mutex<RuntimeMetrics>>, error: String) {
    log::error!("Audio stream error: {}", error);
    if let Ok(mut runtime) = runtime.lock() {
        runtime.phase = AudioHealthPhase::Error;
        runtime.last_error = Some(error);
        runtime.phase_changed_ms = now_millis();
    }
}

impl Default for AudioStreamer {
    fn default() -> Self {
        Self::new()
    }
}

fn process_input_data(
    data: &[f32],
    channels: u16,
    is_recording: Arc<AtomicBool>,
    accumulated: Arc<Mutex<Vec<f32>>>,
    live_level: Arc<AtomicU32>,
    runtime: Arc<Mutex<RuntimeMetrics>>,
) {
    if !is_recording.load(Ordering::SeqCst) {
        live_level.store(0.0_f32.to_bits(), Ordering::Relaxed);
        return;
    }

    let mono = if channels <= 1 {
        data.to_vec()
    } else {
        data.chunks(channels as usize)
            .map(|frame| frame.iter().copied().sum::<f32>() / frame.len() as f32)
            .collect::<Vec<_>>()
    };

    if !mono.is_empty() {
        let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len() as f32).sqrt();
        let peak = mono
            .iter()
            .map(|s| s.abs())
            .fold(0.0_f32, |acc, value| acc.max(value));
        let active_ratio =
            mono.iter().filter(|sample| sample.abs() > 0.012).count() as f32 / mono.len() as f32;

        let raw_level = if rms < 0.0012 && peak < 0.01 {
            0.0
        } else {
            ((rms * 12.0) + (peak * 1.8) + (active_ratio * 2.2))
                .clamp(0.0, 1.0)
                .powf(0.9)
        };

        let previous = f32::from_bits(live_level.load(Ordering::Relaxed));
        let smoothed = (previous * 0.22 + raw_level * 0.78).clamp(0.0, 1.0);
        live_level.store(smoothed.to_bits(), Ordering::Relaxed);
    }

    if let Ok(mut metrics) = runtime.lock() {
        metrics.phase = AudioHealthPhase::Healthy;
        metrics.last_callback_ms = Some(now_millis());
        metrics.last_error = None;
    }

    if let Ok(mut samples) = accumulated.lock() {
        samples.extend_from_slice(&mono);
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_owner_shutdown_signals_and_joins_owner_thread() {
        let (stop_tx, stop_rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_on_thread = stopped.clone();
        let thread = thread::spawn(move || {
            let _ = stop_rx.recv();
            stopped_on_thread.store(true, Ordering::Release);
        });

        let owner = CaptureOwner {
            stop_tx: Some(stop_tx),
            thread: Some(thread),
        };
        drop(owner);

        assert!(stopped.load(Ordering::Acquire));
    }

    #[test]
    fn input_callback_waits_for_sample_buffer_instead_of_dropping_audio() {
        let is_recording = Arc::new(AtomicBool::new(true));
        let accumulated = Arc::new(Mutex::new(Vec::new()));
        let live_level = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
        let runtime = Arc::new(Mutex::new(RuntimeMetrics::default()));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (completed_tx, completed_rx) = mpsc::channel();

        let held_buffer = accumulated.lock().expect("sample buffer lock");
        let callback_recording = is_recording.clone();
        let callback_accumulated = accumulated.clone();
        let callback_level = live_level.clone();
        let callback_runtime = runtime.clone();
        let callback = thread::spawn(move || {
            entered_tx.send(()).expect("signal callback entry");
            process_input_data(
                &[0.2, -0.4, 0.6, -0.8],
                2,
                callback_recording,
                callback_accumulated,
                callback_level,
                callback_runtime,
            );
            completed_tx.send(()).expect("signal callback completion");
        });

        entered_rx.recv().expect("callback entered");
        assert!(
            completed_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "callback must wait for the sample buffer rather than discard frames"
        );
        drop(held_buffer);
        completed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("callback completed after buffer release");
        callback.join().expect("callback thread");

        let samples = accumulated.lock().expect("sample buffer");
        assert_eq!(samples.len(), 2);
        for sample in samples.iter() {
            assert!(
                (sample + 0.1).abs() < 1.0e-6,
                "unexpected mono sample {sample}"
            );
        }
    }
}
