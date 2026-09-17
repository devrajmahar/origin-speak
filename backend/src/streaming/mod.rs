//! Real-time audio streaming module
//!
//! Captures microphone audio and accumulates it for processing.
//! Cross-platform support for Windows, macOS, and Linux.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex,
};
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
    // Store stream handle to keep it alive
    stream_handle: Arc<Mutex<Option<StreamHandle>>>,
    runtime: Arc<Mutex<RuntimeMetrics>>,
}

/// Wrapper to hold the stream (cpal::Stream is not Send on some platforms)
struct StreamHandle {
    _stream: cpal::Stream,
}

// Safety: We ensure the stream is only accessed from the thread that created it
unsafe impl Send for StreamHandle {}
unsafe impl Sync for StreamHandle {}

impl AudioStreamer {
    pub fn new() -> Self {
        Self {
            is_recording: Arc::new(AtomicBool::new(false)),
            accumulated_samples: Arc::new(Mutex::new(Vec::with_capacity(
                SAMPLE_RATE as usize * 30,
            ))),
            live_level_bits: Arc::new(AtomicU32::new(0.0_f32.to_bits())),
            stream_handle: Arc::new(Mutex::new(None)),
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

    /// Start recording audio directly to internal buffer
    pub fn start_streaming(&self, preferred_device_name: Option<&str>) -> Result<(), String> {
        if self.is_recording.load(Ordering::SeqCst) {
            return Err("Already recording".to_string());
        }

        // Clear previous samples
        if let Ok(mut samples) = self.accumulated_samples.lock() {
            samples.clear();
        }
        self.live_level_bits
            .store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.update_runtime(AudioHealthPhase::Starting, None, None, false);

        let is_recording = self.is_recording.clone();
        let accumulated = self.accumulated_samples.clone();
        let live_level = self.live_level_bits.clone();
        let stream_handle = self.stream_handle.clone();
        let runtime = self.runtime.clone();

        is_recording.store(true, Ordering::SeqCst);

        // Build stream on current thread (important for macOS)
        let host = cpal::default_host();

        log::info!("Audio host: {}", host.id().name());

        let device = match Self::select_input_device(&host, preferred_device_name) {
            Ok(d) => d,
            Err(e) => {
                is_recording.store(false, Ordering::SeqCst);
                return Err(e);
            }
        };

        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
        log::info!("Using input device: {}", device_name);
        self.update_runtime(
            AudioHealthPhase::Starting,
            Some(device_name.clone()),
            None,
            false,
        );

        // Use the device's native shared-mode format first; this is less likely to
        // trigger routing changes or device lockups than forcing a custom capture format.
        let supported_config = device
            .default_input_config()
            .map_err(|e| format!("Failed to get default input config: {}", e))?;

        log::info!("Default config: {:?}", supported_config);
        if let Ok(mut runtime) = self.runtime.lock() {
            runtime.sample_rate_hz = supported_config.sample_rate().0;
        }

        let is_rec = is_recording.clone();
        let acc = accumulated.clone();
        let config: cpal::StreamConfig = supported_config.clone().into();

        let build_stream =
            |err_runtime: Arc<Mutex<RuntimeMetrics>>| match supported_config.sample_format() {
                cpal::SampleFormat::F32 => device.build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        process_input_data(
                            data,
                            config.channels,
                            is_rec.clone(),
                            acc.clone(),
                            live_level.clone(),
                            runtime.clone(),
                        );
                    },
                    move |err| {
                        log::error!("Audio stream error: {}", err);
                        if let Ok(mut runtime) = err_runtime.lock() {
                            runtime.phase = AudioHealthPhase::Error;
                            runtime.last_error = Some(err.to_string());
                            runtime.phase_changed_ms = now_millis();
                        }
                    },
                    None,
                ),
                cpal::SampleFormat::I16 => device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let converted: Vec<f32> =
                            data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                        process_input_data(
                            &converted,
                            config.channels,
                            is_rec.clone(),
                            acc.clone(),
                            live_level.clone(),
                            runtime.clone(),
                        );
                    },
                    move |err| {
                        log::error!("Audio stream error: {}", err);
                        if let Ok(mut runtime) = err_runtime.lock() {
                            runtime.phase = AudioHealthPhase::Error;
                            runtime.last_error = Some(err.to_string());
                            runtime.phase_changed_ms = now_millis();
                        }
                    },
                    None,
                ),
                cpal::SampleFormat::U16 => device.build_input_stream(
                    &config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        let converted: Vec<f32> = data
                            .iter()
                            .map(|s| (*s as f32 / u16::MAX as f32) * 2.0 - 1.0)
                            .collect();
                        process_input_data(
                            &converted,
                            config.channels,
                            is_rec.clone(),
                            acc.clone(),
                            live_level.clone(),
                            runtime.clone(),
                        );
                    },
                    move |err| {
                        log::error!("Audio stream error: {}", err);
                        if let Ok(mut runtime) = err_runtime.lock() {
                            runtime.phase = AudioHealthPhase::Error;
                            runtime.last_error = Some(err.to_string());
                            runtime.phase_changed_ms = now_millis();
                        }
                    },
                    None,
                ),
                _other => Err(cpal::BuildStreamError::StreamConfigNotSupported),
            };

        let stream = build_stream(self.runtime.clone()).map_err(|e| {
            is_recording.store(false, Ordering::SeqCst);
            self.update_runtime(
                AudioHealthPhase::Error,
                Some(device_name.clone()),
                Some(format!("Failed to build audio stream: {e}")),
                false,
            );
            format!(
                "Failed to build audio stream: {}. Check microphone permissions.",
                e
            )
        })?;

        // Start the stream
        stream.play().map_err(|e| {
            is_recording.store(false, Ordering::SeqCst);
            self.update_runtime(
                AudioHealthPhase::Error,
                Some(device_name.clone()),
                Some(format!("Failed to start audio stream: {e}")),
                false,
            );
            format!("Failed to start audio stream: {}", e)
        })?;

        // Store stream handle to keep it alive
        if let Ok(mut handle) = stream_handle.lock() {
            *handle = Some(StreamHandle { _stream: stream });
        }

        log::info!("Audio streaming started at {} Hz", config.sample_rate.0);

        Ok(())
    }

    /// Stop streaming
    pub fn stop_streaming(&self) {
        self.is_recording.store(false, Ordering::SeqCst);
        self.live_level_bits
            .store(0.0_f32.to_bits(), Ordering::Relaxed);

        // Dropping the input stream releases the capture handle more reliably than
        // pausing first on some Windows audio stacks.
        let active_stream = self
            .stream_handle
            .lock()
            .ok()
            .and_then(|mut handle| handle.take());
        drop(active_stream);

        self.update_runtime(AudioHealthPhase::Idle, None, None, false);

        log::info!("Audio streaming stopped");
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

    if let Ok(mut samples) = accumulated.try_lock() {
        samples.extend_from_slice(&mono);
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
