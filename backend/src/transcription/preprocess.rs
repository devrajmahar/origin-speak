//! Shared audio conditioning for every local transcription backend.

use std::f32::consts::PI;

const HIGH_PASS_HZ: f32 = 80.0;
const VAD_FRAME_MS: usize = 20;
const VAD_MARGIN_MS: usize = 150;
const PAD_MS: usize = 200;
const TARGET_RMS: f32 = 0.10;
const TARGET_PEAK: f32 = 0.707_945_76; // -3 dBFS
const MAX_GAIN: f32 = 10.0; // +20 dB

pub(crate) fn condition_audio(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    if samples.is_empty() || sample_rate == 0 {
        return Vec::new();
    }

    let mut filtered = remove_dc_and_high_pass(samples, sample_rate);
    let speech = trim_silence(&filtered, sample_rate);
    filtered = speech.to_vec();
    normalize(&mut filtered);

    let padding = samples_for_ms(sample_rate, PAD_MS);
    let mut output = Vec::with_capacity(filtered.len() + padding * 2);
    output.resize(padding, 0.0);
    output.extend(filtered);
    output.resize(output.len() + padding, 0.0);
    output
}

pub(crate) fn downmix_interleaved(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let complete_len = samples.len() / channels * channels;
    if complete_len == 0 {
        return Vec::new();
    }

    let frames = complete_len / channels;
    let mut rms = vec![0.0_f32; channels];
    for frame in samples[..complete_len].chunks_exact(channels) {
        for (channel, sample) in frame.iter().enumerate() {
            rms[channel] += sample * sample;
        }
    }
    for value in &mut rms {
        *value = (*value / frames as f32).sqrt();
    }
    let loudest = rms.iter().copied().fold(0.0_f32, f32::max);
    let live = rms
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (loudest == 0.0 || *value >= loudest * 0.1).then_some(index))
        .collect::<Vec<_>>();

    samples[..complete_len]
        .chunks_exact(channels)
        .map(|frame| live.iter().map(|index| frame[*index]).sum::<f32>() / live.len() as f32)
        .collect()
}

fn remove_dc_and_high_pass(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    let dt = 1.0 / sample_rate as f32;
    let rc = 1.0 / (2.0 * PI * HIGH_PASS_HZ);
    let alpha = rc / (rc + dt);
    let mut previous_input = samples[0] - mean;
    let mut previous_output = 0.0_f32;
    let mut output = Vec::with_capacity(samples.len());
    for sample in samples {
        let input = *sample - mean;
        let value = alpha * (previous_output + input - previous_input);
        output.push(value);
        previous_input = input;
        previous_output = value;
    }
    output
}

fn trim_silence(samples: &[f32], sample_rate: u32) -> &[f32] {
    let frame_len = samples_for_ms(sample_rate, VAD_FRAME_MS).max(1);
    let frame_rms = samples
        .chunks(frame_len)
        .map(|frame| {
            (frame.iter().map(|sample| sample * sample).sum::<f32>() / frame.len() as f32).sqrt()
        })
        .collect::<Vec<_>>();
    let max_rms = frame_rms.iter().copied().fold(0.0_f32, f32::max);
    let threshold = (max_rms * 0.08).max(0.003);
    let Some(first_frame) = frame_rms.iter().position(|rms| *rms >= threshold) else {
        return samples;
    };
    let last_frame = frame_rms
        .iter()
        .rposition(|rms| *rms >= threshold)
        .unwrap_or(first_frame);
    let margin = samples_for_ms(sample_rate, VAD_MARGIN_MS);
    let start = (first_frame * frame_len).saturating_sub(margin);
    let end = ((last_frame + 1) * frame_len)
        .saturating_add(margin)
        .min(samples.len());
    &samples[start..end]
}

fn normalize(samples: &mut [f32]) {
    if samples.is_empty() {
        return;
    }
    let rms =
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt();
    let peak = samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    if rms <= f32::EPSILON || peak <= f32::EPSILON {
        return;
    }
    let gain = (TARGET_RMS / rms).min(TARGET_PEAK / peak).min(MAX_GAIN);
    for sample in samples {
        *sample = (*sample * gain).clamp(-1.0, 1.0);
    }
}

fn samples_for_ms(sample_rate: u32, milliseconds: usize) -> usize {
    sample_rate as usize * milliseconds / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn dc_and_sub_bass_are_attenuated() {
        let rate = 16_000;
        let input = (0..rate)
            .map(|index| 0.25 + 0.2 * (2.0 * PI * 20.0 * index as f32 / rate as f32).sin())
            .collect::<Vec<_>>();
        let output = remove_dc_and_high_pass(&input, rate);
        assert!(output.iter().sum::<f32>().abs() / (output.len() as f32) < 0.001);
        assert!(rms(&output[rate as usize / 2..]) < 0.05);
    }

    #[test]
    fn silent_channel_is_not_averaged_into_live_channel() {
        let stereo = [0.4, 0.0, -0.4, 0.0, 0.2, 0.0];
        assert_eq!(downmix_interleaved(&stereo, 2), vec![0.4, -0.4, 0.2]);
    }

    #[test]
    fn balanced_channels_are_averaged() {
        let stereo = [0.4, 0.2, -0.4, -0.2];
        assert_eq!(downmix_interleaved(&stereo, 2), vec![0.3, -0.3]);
    }

    #[test]
    fn vad_keeps_margin_and_conditioner_adds_padding() {
        let rate = 16_000;
        let mut input = vec![0.0; rate as usize];
        input[rate as usize / 2..rate as usize * 3 / 4].fill(0.2);
        let output = condition_audio(&input, rate);
        let expected_speech_and_margin_ms = 250 + VAD_MARGIN_MS * 2;
        let expected_ms = expected_speech_and_margin_ms + PAD_MS * 2;
        let actual_ms = output.len() * 1000 / rate as usize;
        assert!((actual_ms as isize - expected_ms as isize).abs() <= VAD_FRAME_MS as isize * 2);
        assert!(output[..samples_for_ms(rate, PAD_MS)]
            .iter()
            .all(|sample| *sample == 0.0));
    }

    #[test]
    fn normalization_obeys_gain_and_peak_caps() {
        let mut quiet = vec![0.001, -0.001];
        normalize(&mut quiet);
        assert!((quiet[0] - 0.01).abs() < 0.0001);

        let mut loud = vec![1.0, -1.0, 0.5];
        normalize(&mut loud);
        let peak = loud.iter().copied().map(f32::abs).fold(0.0, f32::max);
        assert!(peak <= TARGET_PEAK + 0.0001);
    }
}
