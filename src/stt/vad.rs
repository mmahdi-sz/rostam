use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, warn};
use wavekat_vad::VoiceActivityDetector;
use wavekat_vad::backends::firered::FireRedVad;

/// Finds split points in audio using FireRedVAD to avoid Vosk lattice explosion on long files.
///
/// Returns a vector of sample index ranges `(start, end)` representing chunks of audio.
/// Chunks target 30–45 seconds, splitting at detected silence gaps (prob < 0.35).
/// If total duration <= 45s, returns a single range `[(0, samples.len())]`.
pub fn get_speech_chunks(
    samples: &[i16],
    sample_rate: u32,
    cancel_flag: Option<&Arc<AtomicBool>>,
) -> Vec<(usize, usize)> {
    let total_samples = samples.len();
    let threshold_samples = (sample_rate as usize) * 45;

    // Fast-path: short audio doesn't need chunking
    if total_samples <= threshold_samples {
        return vec![(0, total_samples)];
    }

    // Try VAD-based splitting
    match split_with_vad(samples, sample_rate, cancel_flag) {
        Ok(chunks) if !chunks.is_empty() => chunks,
        Ok(_) => split_with_fallback(samples, sample_rate),
        Err(e) => {
            warn!("[stt] VAD splitting failed, using fallback chunker: {e}");
            split_with_fallback(samples, sample_rate)
        }
    }
}

fn split_with_vad(
    samples: &[i16],
    sample_rate: u32,
    cancel_flag: Option<&Arc<AtomicBool>>,
) -> anyhow::Result<Vec<(usize, usize)>> {
    let mut vad =
        FireRedVad::new().map_err(|e| anyhow::anyhow!("FireRedVAD init failed: {e:?}"))?;
    let caps = vad.capabilities();
    let frame_size = caps.frame_size; // typically 160 samples (10ms at 16kHz)

    if frame_size == 0 {
        anyhow::bail!("Invalid frame size 0 from VAD capabilities");
    }

    let min_chunk_samples = (sample_rate as usize) * 25; // 25s
    let target_chunk_samples = (sample_rate as usize) * 35; // 35s
    let max_chunk_samples = (sample_rate as usize) * 50; // 50s

    let mut chunks = Vec::new();
    let mut chunk_start = 0;
    let mut best_silence_point: Option<usize> = None;
    let mut best_silence_prob = 1.0f32;
    let mut consecutive_silence_frames = 0;

    let total_samples = samples.len();
    let mut frame_start = 0;

    while frame_start + frame_size <= total_samples {
        if let Some(flag) = cancel_flag {
            if flag.load(Ordering::Relaxed) {
                anyhow::bail!("STT cancelled during VAD");
            }
        }

        let frame = &samples[frame_start..frame_start + frame_size];
        let prob = vad.process(frame, sample_rate).unwrap_or(0.5);

        let current_chunk_len = frame_start - chunk_start;

        if prob < 0.35 {
            consecutive_silence_frames += 1;
        } else {
            consecutive_silence_frames = 0;
        }

        if current_chunk_len >= min_chunk_samples {
            // Track the frame with lowest probability in case we hit max_chunk_samples
            if prob < best_silence_prob {
                best_silence_prob = prob;
                best_silence_point = Some(frame_start + frame_size / 2);
            }

            // Ideal split: at least 3 consecutive silence frames (30ms silence) after reaching target
            let is_ideal_silence =
                consecutive_silence_frames >= 3 && current_chunk_len >= target_chunk_samples;
            let is_emergency_split = current_chunk_len >= max_chunk_samples;

            if is_ideal_silence || is_emergency_split {
                let cut_point = if is_ideal_silence {
                    frame_start + frame_size / 2
                } else {
                    best_silence_point.unwrap_or(frame_start)
                };

                // Avoid zero-length or degenerate chunks
                if cut_point > chunk_start {
                    chunks.push((chunk_start, cut_point));
                    chunk_start = cut_point;
                    best_silence_point = None;
                    best_silence_prob = 1.0;
                    consecutive_silence_frames = 0;
                    vad.reset();
                }
            }
        }

        frame_start += frame_size;
    }

    if chunk_start < total_samples {
        chunks.push((chunk_start, total_samples));
    }

    info!(
        "[stt] VAD split {}s audio into {} chunks (avg {:.1}s/chunk)",
        total_samples as f64 / sample_rate as f64,
        chunks.len(),
        (total_samples as f64 / sample_rate as f64) / chunks.len().max(1) as f64
    );

    Ok(chunks)
}

/// Fallback chunker when VAD is unavailable: splits at fixed 35-second intervals.
fn split_with_fallback(samples: &[i16], sample_rate: u32) -> Vec<(usize, usize)> {
    let chunk_size = (sample_rate as usize) * 35;
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < samples.len() {
        let end = (start + chunk_size).min(samples.len());
        chunks.push((start, end));
        start = end;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_audio_not_chunked() {
        let samples = vec![0i16; 16000 * 30]; // 30s
        let chunks = get_speech_chunks(&samples, 16000, None);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], (0, samples.len()));
    }

    #[test]
    fn test_fallback_chunker() {
        let samples = vec![0i16; 16000 * 100]; // 100s
        let chunks = split_with_fallback(&samples, 16000);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], (0, 16000 * 35));
        assert_eq!(chunks[1], (16000 * 35, 16000 * 70));
        assert_eq!(chunks[2], (16000 * 70, 16000 * 100));
    }

    #[test]
    fn test_vad_live_splitting() {
        // Create 60 seconds of 16kHz mono audio:
        // 0..32s: loud tone (simulating speech)
        // 32..36s: silence (zeros)
        // 36..60s: loud tone
        let mut samples = Vec::with_capacity(16000 * 60);
        for i in 0..(16000 * 60) {
            let t = i as f64 / 16000.0;
            if (32.0..=36.0).contains(&t) {
                samples.push(0i16);
            } else {
                // 440Hz sine wave
                let s = (t * 440.0 * 2.0 * std::f64::consts::PI).sin();
                samples.push((s * 15000.0) as i16);
            }
        }

        let chunks = get_speech_chunks(&samples, 16000, None);
        // Should have split into at least 2 chunks around the silence
        assert!(
            chunks.len() >= 2,
            "Expected multiple chunks, got {:?}",
            chunks
        );
        println!("VAD chunks generated: {:?}", chunks);
    }
}
