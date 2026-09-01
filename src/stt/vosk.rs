use std::io::Read;
use std::time::Instant;

use crate::stt::types::SttConfig;

pub fn transcribe(config: &SttConfig, wav_path: &str) -> crate::error::Result<(String, f64)> {
    let model_dir = config.model_path("files/models/vosk");

    let model =
        vosk::Model::new(&model_dir).ok_or_else(|| anyhow::anyhow!("Vosk model load failed"))?;

    let mut recognizer = vosk::Recognizer::new(&model, 16000.0)
        .ok_or_else(|| anyhow::anyhow!("Vosk Recognizer creation failed"))?;

    let mut wav = std::fs::File::open(wav_path)?;
    let mut header = [0u8; 44];
    wav.read_exact(&mut header)?;

    let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let _byte_rate = u32::from_le_bytes([header[28], header[29], header[30], header[31]]);
    let channels = u16::from_le_bytes([header[22], header[23]]);
    let bits_per_sample = u16::from_le_bytes([header[34], header[35]]);

    if sample_rate != 16000 || channels != 1 || bits_per_sample != 16 {
        anyhow::bail!(
            "Audio must be 16000Hz mono 16-bit PCM (got {sample_rate}Hz {channels}ch {bits_per_sample}bits)"
        );
    }

    let data_len = u32::from_le_bytes([header[40], header[41], header[42], header[43]]) as usize;

    let mut raw = vec![0u8; data_len.min(usize::MAX - 1)];
    let n = wav.read(&mut raw)?;
    raw.truncate(n);
    let samples: Vec<i16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();

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
