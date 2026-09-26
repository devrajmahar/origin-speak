use origin_speak_lib::{
    downmix_audio_for_transcription, format_transcription_for_delivery, TranscriptionService,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::runtime::Builder;

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let model = args.next().ok_or_else(usage)?;
    let directory = PathBuf::from(args.next().ok_or_else(usage)?);
    let options = args.collect::<Vec<_>>();
    let gpu = !options.iter().any(|argument| argument == "--cpu");
    let language = options
        .windows(2)
        .find(|pair| pair[0] == "--language")
        .map(|pair| pair[1].clone())
        .unwrap_or_else(|| "en".to_string());
    Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not create evaluator runtime: {error}"))?
        .block_on(evaluate(&model, &directory, gpu, &language))
}

fn usage() -> String {
    "usage: cargo run --manifest-path backend/Cargo.toml --example evaluate -- <model-id> <wav-directory> [--cpu] [--language <code>]".to_string()
}

async fn evaluate(model: &str, directory: &Path, gpu: bool, language: &str) -> Result<(), String> {
    if !directory.is_dir() {
        return Err(format!(
            "WAV directory does not exist: {}",
            directory.display()
        ));
    }
    let service = TranscriptionService::for_evaluation(model, gpu)?;
    service.ensure_ready()?;
    let mut wavs = fs::read_dir(directory)
        .map_err(|error| format!("could not read {}: {error}", directory.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
        })
        .collect::<Vec<_>>();
    wavs.sort();
    if wavs.is_empty() {
        return Err(format!("no WAV files found in {}", directory.display()));
    }

    println!("file\taudio_ms\tlatency_ms\trtf\twer\ttranscript");
    let (mut errors, mut reference_words, mut referenced_files) = (0usize, 0usize, 0usize);
    let (mut audio_total, mut latency_total) = (0u128, 0u128);
    for path in wavs {
        let (samples, sample_rate) = read_pcm16_wav(&path)?;
        let audio_ms = samples.len() as u128 * 1000 / sample_rate as u128;
        let started = Instant::now();
        let raw = service
            .transcribe(samples, sample_rate, Some(language.to_string()), Vec::new())
            .await?;
        let transcript = format_transcription_for_delivery(&raw, Some(language));
        let latency_ms = started.elapsed().as_millis();
        let reference_path = path.with_extension("txt");
        let wer = if reference_path.is_file() {
            let reference = fs::read_to_string(&reference_path).map_err(|error| {
                format!(
                    "could not read reference {}: {error}",
                    reference_path.display()
                )
            })?;
            let expected = normalized_words(&reference);
            let actual = normalized_words(&transcript);
            let file_errors = word_distance(&expected, &actual);
            errors += file_errors;
            reference_words += expected.len();
            referenced_files += 1;
            format!(
                "{:.2}%",
                file_errors as f64 * 100.0 / expected.len().max(1) as f64
            )
        } else {
            "n/a".to_string()
        };
        audio_total += audio_ms;
        latency_total += latency_ms;
        println!(
            "{}\t{}\t{}\t{:.3}\t{}\t{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            audio_ms,
            latency_ms,
            latency_ms as f64 / audio_ms.max(1) as f64,
            wer,
            transcript.replace(['\t', '\n', '\r'], " ")
        );
    }
    let aggregate_wer = if reference_words == 0 {
        "pending (no sibling .txt references)".to_string()
    } else {
        format!("{:.2}%", errors as f64 * 100.0 / reference_words as f64)
    };
    println!(
        "aggregate: referenced_files={} wer={} audio_ms={} latency_ms={} rtf={:.3}",
        referenced_files,
        aggregate_wer,
        audio_total,
        latency_total,
        latency_total as f64 / audio_total.max(1) as f64
    );
    Ok(())
}

fn read_pcm16_wav(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let bytes =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("{} is not a RIFF/WAVE file", path.display()));
    }
    let (mut format, mut data) = (None, None);
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let length = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let start = offset + 8;
        let end = start.saturating_add(length);
        if end > bytes.len() {
            break;
        }
        if id == b"fmt " && length >= 16 {
            format = Some((
                u16::from_le_bytes(bytes[start..start + 2].try_into().unwrap()),
                u16::from_le_bytes(bytes[start + 2..start + 4].try_into().unwrap()),
                u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap()),
                u16::from_le_bytes(bytes[start + 14..start + 16].try_into().unwrap()),
            ));
        } else if id == b"data" {
            data = Some(&bytes[start..end]);
        }
        offset = end + (length & 1);
    }
    let (encoding, channels, rate, bits) =
        format.ok_or_else(|| "WAV has no fmt chunk".to_string())?;
    if encoding != 1 || bits != 16 || channels == 0 || rate == 0 {
        return Err(format!("{} must be PCM16 WAV", path.display()));
    }
    let interleaved = data
        .ok_or_else(|| "WAV has no data chunk".to_string())?
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / i16::MAX as f32)
        .collect::<Vec<_>>();
    let mono = downmix_audio_for_transcription(&interleaved, channels as usize);
    Ok((mono, rate))
}

fn normalized_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|character| character.is_alphanumeric())
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn word_distance(reference: &[String], hypothesis: &[String]) -> usize {
    let mut previous = (0..=hypothesis.len()).collect::<Vec<_>>();
    for (row, reference_word) in reference.iter().enumerate() {
        let mut current = vec![row + 1; hypothesis.len() + 1];
        for (column, hypothesis_word) in hypothesis.iter().enumerate() {
            current[column + 1] = (previous[column + 1] + 1)
                .min(current[column] + 1)
                .min(previous[column] + usize::from(reference_word != hypothesis_word));
        }
        previous = current;
    }
    previous[hypothesis.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_word_levenshtein_counts_edits() {
        assert_eq!(
            word_distance(
                &normalized_words("Hello, brave new world!"),
                &normalized_words("hello new worlds")
            ),
            2
        );
    }
}
