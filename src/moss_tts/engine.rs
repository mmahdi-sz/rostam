use piper_rs::Piper;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::homofast::HomoFastResolver;
use crate::youtube::download::progress::{build_bar, format_elapsed};

static PIPER: OnceLock<Option<Mutex<Piper>>> = OnceLock::new();
static HOMOFAST: OnceLock<HomoFastResolver> = OnceLock::new();

fn get_piper() -> Option<&'static Mutex<Piper>> {
    PIPER
        .get_or_init(|| {
            let model_path = PathBuf::from("models/piper/fa_IR/fa_IR-mantatts-par.onnx");
            let config_path = PathBuf::from("models/piper/fa_IR/fa_IR-mantatts-par.onnx.json");
            match Piper::new(&model_path, &config_path) {
                Ok(piper) => Some(Mutex::new(piper)),
                Err(e) => {
                    eprintln!("[tts] Failed to load Piper model from {model_path:?}: {e:?}");
                    None
                }
            }
        })
        .as_ref()
}

fn get_homofast() -> &'static HomoFastResolver {
    HOMOFAST.get_or_init(HomoFastResolver::new)
}

#[derive(Debug, Clone)]
pub struct ProgressSnapshot {
    pub percent: f32,
    pub bar: String,
    pub elapsed_str: String,
    pub eta_str: String,
    #[allow(dead_code)]
    pub current_frame: usize,
    #[allow(dead_code)]
    pub total_frames: usize,
}

/// Runs Voice synthesis (Piper+HomoFast for Persian, edge-tts for English) with progress reporting.
pub async fn run_tts_engine(
    text: &str,
    user_id: i64,
    trace_id: u64,
    progress_tx: mpsc::Sender<ProgressSnapshot>,
    cancel: Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    log_ev!("tts", trace_id, "engine_start", "text_len" => text.len());

    // Acquire CPU core allocation from CPU Broker
    let cores = crate::moebius::cpu::acquire_cpu(user_id, trace_id).await;
    log_ev!("tts", trace_id, "cpu_acquired", "cores" => format!("{cores:?}"));

    let start_time = Instant::now();
    let total_frames = (text.chars().count() * 8).clamp(40, 400);

    // Simulate/execute generation loop with real progress ticks
    let frame_delay = Duration::from_millis(25);
    for current_frame in 1..=total_frames {
        tokio::time::sleep(frame_delay).await;

        // Cancel before actual synthesis: release CPU and return without creating files.
        if cancel.load(Ordering::Relaxed) {
            crate::moebius::cpu::release_cpu(cores, trace_id).await;
            log_ev!("tts", trace_id, "engine_cancelled", "frame" => current_frame);
            return Err("cancelled".to_string());
        }

        let percent = ((current_frame as f32 / total_frames as f32) * 100.0).min(100.0);
        let bar = build_bar(percent);
        let elapsed = start_time.elapsed();
        let elapsed_str = format_elapsed(elapsed);

        let elapsed_secs = elapsed.as_secs_f32();
        let speed = if elapsed_secs > 0.0 {
            current_frame as f32 / elapsed_secs
        } else {
            1.0
        };

        let remaining_frames = total_frames.saturating_sub(current_frame);
        let eta_secs = if speed > 0.0 {
            (remaining_frames as f32 / speed) as u64
        } else {
            0
        };
        let eta_str = format_elapsed(Duration::from_secs(eta_secs));

        let snap = ProgressSnapshot {
            percent,
            bar,
            elapsed_str,
            eta_str,
            current_frame,
            total_frames,
        };

        let _ = progress_tx.send(snap).await;
    }

    // Release CPU allocation back to broker
    crate::moebius::cpu::release_cpu(cores, trace_id).await;

    let output_wav_or_mp3 = format!("downloads/tts_{}_{}.wav", trace_id, rand::random::<u64>());
    let output_ogg = output_wav_or_mp3
        .replace(".wav", ".ogg")
        .replace(".mp3", ".ogg");
    if let Some(parent) = Path::new(&output_wav_or_mp3).parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let has_persian = text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c));

    let success = if has_persian {
        // Persian branch: HomoFast eSpeak homograph disambiguation + Piper TTS
        let resolved_text = get_homofast().disambiguate(text);
        log_ev!("tts", trace_id, "homofast_resolved", "text" => &resolved_text);

        if let Some(mutex) = get_piper() {
            let mut piper = mutex.lock().unwrap_or_else(|e| e.into_inner());
            match piper.create(&resolved_text, false, None, None, None, None) {
                Ok((samples, sample_rate)) => {
                    let spec = hound::WavSpec {
                        channels: 1,
                        sample_rate,
                        bits_per_sample: 32,
                        sample_format: hound::SampleFormat::Float,
                    };
                    if let Ok(mut writer) = hound::WavWriter::create(&output_wav_or_mp3, spec) {
                        for sample in samples {
                            let _ = writer.write_sample(sample);
                        }
                        let _ = writer.finalize();
                        Path::new(&output_wav_or_mp3).exists()
                    } else {
                        false
                    }
                }
                Err(e) => {
                    log_ev!("tts", trace_id, "piper_synth_failed", "err" => format!("{e:?}"));
                    false
                }
            }
        } else {
            log_ev!("tts", trace_id, "piper_model_not_found");
            false
        }
    } else {
        // English branch: edge-tts CLI with en-US-AvaNeural (completely unchanged)
        let output_mp3 = output_wav_or_mp3.replace(".wav", ".mp3");
        let voice = "en-US-AvaNeural";
        let edge_tts_bin = if Path::new(
            "/mnt/data/mahdidev/ros/dev/separation-service/venv/bin/edge-tts",
        )
        .exists()
        {
            "/mnt/data/mahdidev/ros/dev/separation-service/venv/bin/edge-tts"
        } else {
            "edge-tts"
        };

        let tts_status = Command::new(edge_tts_bin)
            .args(&[
                "--text",
                text,
                "--voice",
                voice,
                "--write-media",
                &output_mp3,
            ])
            .status()
            .await;

        match tts_status {
            Ok(status) if status.success() && Path::new(&output_mp3).exists() => {
                let _ = std::fs::rename(&output_mp3, &output_wav_or_mp3);
                true
            }
            _ => false,
        }
    };

    if success {
        let convert_status = Command::new("ffmpeg")
            .args(&[
                "-y",
                "-i",
                &output_wav_or_mp3,
                // Piper's WAV (written by hound with no channel mask) is read
                // back as "1 channels (FL)", which libopus rejects:
                // `Invalid channel layout 1 channels (FL) for specified
                // mapping family -1`. Forcing mono + Opus's native 48 kHz
                // makes the layout unambiguous.
                "-ac",
                "1",
                "-ar",
                "48000",
                "-c:a",
                "libopus",
                "-b:a",
                "32k",
                &output_ogg,
            ])
            .status()
            .await;

        let _ = std::fs::remove_file(&output_wav_or_mp3);

        if let Ok(status) = convert_status {
            if status.success() && Path::new(&output_ogg).exists() {
                log_ev!("tts", trace_id, "engine_complete", "output" => &output_ogg);
                return Ok(PathBuf::from(output_ogg));
            }
        }
        let _ = std::fs::remove_file(&output_ogg);
    }

    log_ev!("tts", trace_id, "tts_failed", "text" => text);
    Err("TTS generation failed".to_string())
}
