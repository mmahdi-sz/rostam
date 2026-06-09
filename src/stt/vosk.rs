use std::time::Instant;
use std::io::Read;

use crate::stt::types::SttConfig;

pub fn transcribe(
    config: &SttConfig,
    wav_path: &str,
) -> Result<(String, f64), Box<dyn std::error::Error>> {
    let model_dir = config.model_path("files/models/vosk");

    let model = vosk::Model::new(&model_dir)
        .ok_or_else(|| "Vosk model load failed")?;

    let mut recognizer = vosk::Recognizer::new(&model, 16000.0)
        .ok_or_else(|| "Vosk Recognizer creation failed")?;

    let mut wav = std::fs::File::open(wav_path)?;
    let mut header = [0u8; 44];
    wav.read_exact(&mut header)?;

    let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let byte_rate = u32::from_le_bytes([header[28], header[29], header[30], header[31]]);
    let channels = u16::from_le_bytes([header[22], header[23]]);
    let bits_per_sample = u16::from_le_bytes([header[34], header[35]]);

    if sample_rate != 16000 || channels != 1 || bits_per_sample != 16 {
        return Err(format!(
            "Audio must be 16000Hz mono 16-bit PCM (got {}Hz {}ch {}bit)",
            sample_rate, channels, bits_per_sample
        ).into());
    }

    let data_len = u32::from_le_bytes([header[40], header[41], header[42], header[43]]) as usize;

    let mut raw = vec![0u8; data_len.min(usize::MAX - 1)];
    let n = wav.read(&mut raw)?;
    raw.truncate(n);
    let samples: Vec<i16> = raw.chunks_exact(2)
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
    let text = result.single().map(|s| s.text.to_string()).unwrap_or_default();
    let elapsed = start.elapsed().as_secs_f64();

    Ok((text, elapsed))
}
