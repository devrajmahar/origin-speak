//! Audio capture and processing module
//!
//! Handles microphone input capture using the `cpal` library.

use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Audio device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDevice {
    pub name: String,
    pub is_default: bool,
    pub sample_rate: Option<u32>,
}

/// Audio recording state
#[derive(Debug, Default)]
pub struct AudioState {
    /// Raw audio samples captured
    pub samples: Arc<Mutex<Vec<f32>>>,
    /// Whether currently recording
    pub is_recording: bool,
    /// Selected device name
    pub selected_device: Option<String>,
    /// Sample rate
    pub sample_rate: u32,
}

impl AudioState {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            is_recording: false,
            selected_device: None,
            sample_rate: 16000, // Default for Whisper
        }
    }

    /// Get list of available audio input devices
    pub fn get_devices() -> Result<Vec<AudioDevice>, String> {
        let host = cpal::default_host();
        let default_device = host.default_input_device();
        let default_name = default_device.as_ref().and_then(|d| d.name().ok());

        let devices = host
            .input_devices()
            .map_err(|e| format!("Failed to enumerate devices: {}", e))?
            .filter_map(|device| {
                let name = device.name().ok()?;
                let config = device.default_input_config().ok()?;
                Some(AudioDevice {
                    is_default: default_name.as_ref() == Some(&name),
                    name,
                    sample_rate: Some(config.sample_rate().0),
                })
            })
            .collect();

        Ok(devices)
    }

    /// Start recording audio
    pub fn start_recording(&mut self) -> Result<(), String> {
        if self.is_recording {
            return Err("Already recording".to_string());
        }

        // Clear previous samples
        if let Ok(mut samples) = self.samples.lock() {
            samples.clear();
        }

        self.is_recording = true;
        log::info!("Started audio recording");
        Ok(())
    }

    /// Stop recording and return samples
    pub fn stop_recording(&mut self) -> Result<Vec<f32>, String> {
        if !self.is_recording {
            return Err("Not recording".to_string());
        }

        self.is_recording = false;

        let samples = self
            .samples
            .lock()
            .map_err(|e| format!("Failed to lock samples: {}", e))?
            .clone();

        log::info!("Stopped recording. Captured {} samples", samples.len());
        Ok(samples)
    }

    /// Add samples to the buffer
    pub fn add_samples(&self, new_samples: &[f32]) {
        if let Ok(mut samples) = self.samples.lock() {
            samples.extend_from_slice(new_samples);
        }
    }
}
