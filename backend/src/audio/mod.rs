//! Audio device discovery and selection state.

use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDevice {
    pub name: String,
    pub is_default: bool,
    pub sample_rate: Option<u32>,
}

#[derive(Debug, Default)]
pub struct AudioState {
    pub selected_device: Option<String>,
}

impl AudioState {
    pub fn get_devices() -> Result<Vec<AudioDevice>, String> {
        let host = cpal::default_host();
        let default_device = host.default_input_device();
        let default_name = default_device
            .as_ref()
            .and_then(|device| device.name().ok());

        let devices = host
            .input_devices()
            .map_err(|error| format!("Failed to enumerate devices: {error}"))?
            .filter_map(|device| {
                let name = device.name().ok()?;
                let config = device.default_input_config().ok()?;
                Some(AudioDevice {
                    is_default: default_name.as_ref() == Some(&name),
                    name,
                    sample_rate: Some(config.sample_rate().0),
                })
            })
            .collect::<Vec<_>>();
        Ok(devices)
    }
}
