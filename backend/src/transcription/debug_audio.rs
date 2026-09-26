use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static CAPTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn enabled() -> bool {
    std::env::var_os("ORIGIN_SPEAK_DEBUG_AUDIO_DIR").is_some()
}

pub(crate) fn dump_pair_if_enabled(
    raw: &[f32],
    raw_rate: u32,
    model_input: &[f32],
    model_rate: u32,
) {
    let Some(directory) = std::env::var_os("ORIGIN_SPEAK_DEBUG_AUDIO_DIR") else {
        return;
    };
    let directory = Path::new(&directory);
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let sequence = CAPTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let stem = format!("origin-speak-{timestamp_ms}-{sequence:04}");
    let result = fs::create_dir_all(directory).and_then(|_| {
        write_pcm16_mono_wav(
            &directory.join(format!("{stem}-raw-{raw_rate}hz.wav")),
            raw,
            raw_rate,
        )?;
        write_pcm16_mono_wav(
            &directory.join(format!("{stem}-model-{model_rate}hz.wav")),
            model_input,
            model_rate,
        )
    });
    if let Err(error) = result {
        log::warn!("Could not write debug audio capture: {error}");
    }
}

fn write_pcm16_mono_wav(path: &Path, samples: &[f32], sample_rate: u32) -> io::Result<()> {
    let data_bytes = samples.len().saturating_mul(2);
    let data_bytes = u32::try_from(data_bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "WAV capture is too large"))?;
    let mut file = File::create(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&(36_u32.saturating_add(data_bytes)).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&sample_rate.saturating_mul(2).to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_bytes.to_le_bytes())?;
    for sample in samples {
        let pcm = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        file.write_all(&pcm.to_le_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_writer_emits_pcm16_mono_header() {
        let path =
            std::env::temp_dir().join(format!("origin-debug-wav-{}.wav", std::process::id()));
        write_pcm16_mono_wav(&path, &[0.0, 1.0, -1.0], 16_000).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 1);
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            16_000
        );
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 6);
        let _ = std::fs::remove_file(path);
    }
}
