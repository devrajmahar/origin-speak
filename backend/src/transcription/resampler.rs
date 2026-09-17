//! Band-limited conversion to Whisper's 16 kHz input rate.
//!
//! Adapted from VoxType's streaming resampler implementation:
//! https://github.com/peteonrails/voxtype/blob/320a737/src/audio/resampler.rs
//! The important properties retained here are anti-aliasing during downsampling
//! and flushing rubato's delayed tail instead of dropping the end of speech.

use rubato::{FftFixedIn, Resampler};

const CHUNK: usize = 1024;

pub fn resample_buffer(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>, String> {
    if from_rate == 0 || to_rate == 0 {
        return Err(format!("invalid sample rates: {from_rate} -> {to_rate}"));
    }
    if samples.is_empty() || from_rate == to_rate {
        return Ok(samples.to_vec());
    }

    let mut inner = FftFixedIn::<f32>::new(from_rate as usize, to_rate as usize, CHUNK, 1, 1)
        .map_err(|error| format!("could not build resampler {from_rate} -> {to_rate}: {error}"))?;

    let ratio = to_rate as f64 / from_rate as f64;
    let expected = (samples.len() as f64 * ratio).round() as usize;
    let delay = inner.output_delay();
    let mut output = Vec::with_capacity(expected + delay);
    let mut offset = 0usize;

    while offset + CHUNK <= samples.len() {
        let chunk = samples[offset..offset + CHUNK].to_vec();
        let mut converted = inner
            .process(&[chunk], None)
            .map_err(|error| format!("resampler step failed: {error}"))?;
        if let Some(channel) = converted.pop() {
            output.extend(channel);
        }
        offset += CHUNK;
    }

    if offset < samples.len() {
        let mut pending = samples[offset..].to_vec();
        pending.resize(CHUNK, 0.0);
        let mut converted = inner
            .process(&[pending], None)
            .map_err(|error| format!("resampler flush failed: {error}"))?;
        if let Some(channel) = converted.pop() {
            output.extend(channel);
        }
    }

    // FftFixedIn delays the signal by `output_delay()` samples. Feed silence
    // until the real input tail has emerged, then drop the leading delay and
    // return exactly the real-duration sample count.
    let target = expected + delay;
    let output_per_chunk = inner.output_frames_next().max(1);
    let max_flush_chunks = (delay / output_per_chunk).saturating_add(4);
    for _ in 0..max_flush_chunks {
        if output.len() >= target {
            break;
        }
        let mut converted = inner
            .process(&[vec![0.0; CHUNK]], None)
            .map_err(|error| format!("resampler tail flush failed: {error}"))?;
        if let Some(channel) = converted.pop() {
            output.extend(channel);
        }
    }

    if output.len() < target {
        return Err(format!(
            "resampler produced too little output after delay compensation: got {}, need {target}",
            output.len()
        ));
    }

    Ok(output[delay..target].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn tone(freq: f32, rate: u32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|index| (TAU * freq * index as f32 / rate as f32).sin())
            .collect()
    }

    fn energy_at(samples: &[f32], freq: f32, rate: u32) -> f32 {
        let (mut real, mut imaginary) = (0.0_f32, 0.0_f32);
        for (index, sample) in samples.iter().enumerate() {
            let phase = TAU * freq * index as f32 / rate as f32;
            real += sample * phase.cos();
            imaginary += sample * phase.sin();
        }
        ((real * real + imaginary * imaginary).sqrt()) / samples.len() as f32
    }

    #[test]
    fn downsampling_filters_energy_above_nyquist() {
        let input = tone(12_000.0, 48_000, 48_000);
        let output = resample_buffer(&input, 48_000, 16_000).expect("resample");

        let alias = energy_at(&output, 4_000.0, 16_000);
        let source = energy_at(&input, 12_000.0, 48_000);
        assert!(
            alias < source * 0.1,
            "high-frequency energy folded into speech band"
        );
    }

    #[test]
    fn resampling_preserves_duration_and_tail() {
        let input = tone(440.0, 48_000, 48_000 + 317);
        let output = resample_buffer(&input, 48_000, 16_000).expect("resample");
        let expected = (input.len() as f64 / 3.0).round() as usize;
        assert_eq!(output.len(), expected);
        assert!(energy_at(&output, 440.0, 16_000) > 0.2);
    }

    #[test]
    fn resampling_keeps_signal_at_the_end_of_recording() {
        let mut input = vec![0.0_f32; 48_000 + 317];
        let tail_start = input.len() - 96;
        input[tail_start..].fill(0.8);

        let output = resample_buffer(&input, 48_000, 16_000).expect("resample");
        let tail_peak = output[output.len().saturating_sub(96)..]
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);

        assert!(tail_peak > 0.1, "terminal speech-like signal was clipped");
    }

    #[test]
    fn matching_rates_are_unchanged() {
        let input = vec![0.1, -0.2, 0.3];
        assert_eq!(resample_buffer(&input, 16_000, 16_000).unwrap(), input);
    }
}
