#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayState {
    #[default]
    Idle,
    Listening,
    Processing,
    Success,
    Error,
}

/// Starting noise-floor guess on the backend meter scale (about -48 dBFS).
const INITIAL_NOISE_FLOOR: f32 = 0.25;
/// Sound must clear the floor by this much (about 4 dB) to count as voice.
const NOISE_MARGIN: f32 = 0.08;
const VOICE_GAIN: f32 = 1.6;
/// Per meter sample (~45 ms): the floor drops quickly into quiet gaps and
/// creeps up slowly, so continuous speech is not mistaken for room noise.
const FLOOR_FALL_RATE: f32 = 0.5;
const FLOOR_RISE_RATE: f32 = 0.02;

/// Turns the backend's dBFS meter into a 0..1 voice level by removing the
/// room's noise floor, so silence reads as zero regardless of microphone gain.
#[derive(Clone, Copy, Debug)]
pub struct VoiceLevelTracker {
    noise_floor: f32,
}

impl Default for VoiceLevelTracker {
    fn default() -> Self {
        Self {
            noise_floor: INITIAL_NOISE_FLOOR,
        }
    }
}

impl VoiceLevelTracker {
    pub fn update(&mut self, meter: f32) -> f32 {
        // Exactly zero means "no capture data", not a quiet room.
        if meter <= 0.0 {
            return 0.0;
        }
        let meter = meter.min(1.0);
        let rate = if meter < self.noise_floor {
            FLOOR_FALL_RATE
        } else {
            FLOOR_RISE_RATE
        };
        self.noise_floor += (meter - self.noise_floor) * rate;
        let headroom = (1.0 - self.noise_floor - NOISE_MARGIN).max(0.1);
        ((meter - self.noise_floor - NOISE_MARGIN) / headroom * VOICE_GAIN).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_room_noise_reads_as_silence() {
        let mut tracker = VoiceLevelTracker::default();
        let mut level = 1.0;
        for _ in 0..200 {
            level = tracker.update(0.4);
        }
        assert_eq!(level, 0.0);
    }

    #[test]
    fn speech_above_the_floor_reads_as_voice() {
        let mut tracker = VoiceLevelTracker::default();
        for _ in 0..50 {
            tracker.update(0.2);
        }
        assert!(tracker.update(0.45) > 0.3);
        assert!(tracker.update(0.75) > 0.8);
    }

    #[test]
    fn missing_capture_data_does_not_reset_the_floor() {
        let mut tracker = VoiceLevelTracker::default();
        for _ in 0..200 {
            tracker.update(0.4);
        }
        assert_eq!(tracker.update(0.0), 0.0);
        assert_eq!(tracker.update(0.4), 0.0);
    }
}
