use std::time::Instant;

use crate::stt::types::SttConfig;

/// Reads 16000Hz mono 16-bit PCM samples from a WAV file, correctly handling all RIFF chunks.
pub fn read_wav_samples(wav_path: &str) -> crate::error::Result<Vec<i16>> {
    let mut reader = hound::WavReader::open(wav_path)
        .map_err(|e| anyhow::anyhow!("failed to open wav file {wav_path}: {e}"))?;
    let spec = reader.spec();

    if spec.sample_rate != 16000 || spec.channels != 1 || spec.bits_per_sample != 16 {
        anyhow::bail!(
            "Audio must be 16000Hz mono 16-bit PCM (got {}Hz {}ch {}bits)",
            spec.sample_rate,
            spec.channels,
            spec.bits_per_sample
        );
    }

    let samples: Vec<i16> = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("failed to read PCM samples: {e}"))?;

    Ok(samples)
}

pub fn transcribe(config: &SttConfig, wav_path: &str) -> crate::error::Result<(String, f64)> {
    let model_dir = config.model_path("files/models/vosk");

    let model =
        vosk::Model::new(&model_dir).ok_or_else(|| anyhow::anyhow!("Vosk model load failed"))?;

    let mut recognizer = vosk::Recognizer::new(&model, 16000.0)
        .ok_or_else(|| anyhow::anyhow!("Vosk Recognizer creation failed"))?;

    let samples = read_wav_samples(wav_path)?;

    recognizer.set_words(true);

    let start = Instant::now();
    for chunk in samples.chunks(8000) {
        if let Ok(state) = recognizer.accept_waveform(chunk) {
            match state {
                vosk::DecodingState::Finalized => {
                    let _ = recognizer.partial_result();
                }
                vosk::DecodingState::Running | vosk::DecodingState::Failed => {}
            }
        }
    }

    let result = recognizer.final_result();
    let text = result
        .single()
        .map(|s| s.text.to_string())
        .unwrap_or_default();
    let elapsed = start.elapsed().as_secs_f64();

    Ok((text, elapsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_read_wav_samples_with_list_chunk() {
        // Synthesize a valid 16kHz mono 16-bit PCM WAV that contains a LIST chunk before data
        // (matching FFmpeg's standard muxer output)
        let total_samples: u32 = 1600; // 0.1 seconds
        let data_bytes: u32 = total_samples * 2;
        let list_bytes: u32 = 26;
        let riff_len: u32 = 36 + (8 + list_bytes) + data_bytes;

        let temp_dir = std::env::temp_dir();
        let wav_path = temp_dir.join(format!("test_with_list_{}.wav", std::process::id()));
        let mut file = std::fs::File::create(&wav_path).expect("create test wav");

        // RIFF header
        file.write_all(b"RIFF").unwrap();
        file.write_all(&riff_len.to_le_bytes()).unwrap();
        file.write_all(b"WAVE").unwrap();

        // fmt chunk (16 bytes)
        file.write_all(b"fmt \x10\x00\x00\x00").unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
        file.write_all(&1u16.to_le_bytes()).unwrap(); // 1 channel
        file.write_all(&16000u32.to_le_bytes()).unwrap(); // 16000 Hz
        file.write_all(&32000u32.to_le_bytes()).unwrap(); // byte rate
        file.write_all(&2u16.to_le_bytes()).unwrap(); // block align
        file.write_all(&16u16.to_le_bytes()).unwrap(); // 16 bits

        // LIST chunk (26 bytes) - what FFmpeg inserts
        file.write_all(b"LIST").unwrap();
        file.write_all(&list_bytes.to_le_bytes()).unwrap();
        file.write_all(b"INFOISFT\r\0\0\0Lavf63.1.101\0\0").unwrap();

        // data chunk
        file.write_all(b"data").unwrap();
        file.write_all(&data_bytes.to_le_bytes()).unwrap();
        for i in 0..total_samples {
            let sample = (i % 500) as i16;
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
        drop(file);

        let samples = read_wav_samples(wav_path.to_str().unwrap()).expect("read wav samples");
        std::fs::remove_file(&wav_path).ok();
        assert_eq!(samples.len(), total_samples as usize);
        assert_eq!(samples[0], 0);
        assert_eq!(samples[1], 1);
    }
}
