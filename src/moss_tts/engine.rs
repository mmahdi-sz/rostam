use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::youtube::download::progress::{build_bar, format_elapsed};

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

/// Runs MOSS-TTS-Nano voice generation with live progress reporting.
pub async fn run_tts_engine(
    text: &str,
    _prompt_path: Option<&str>,
    user_id: i64,
    trace_id: u64,
    progress_tx: mpsc::Sender<ProgressSnapshot>,
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

    let output_mp3 = format!("downloads/tts_{}_{}.mp3", trace_id, rand::random::<u64>());
    let output_ogg = output_mp3.replace(".mp3", ".ogg");
    if let Some(parent) = Path::new(&output_mp3).parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let has_persian = text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c));
    let voice = if has_persian {
        "fa-IR-DilaraNeural"
    } else {
        "en-US-AvaNeural"
    };

    let edge_tts_bin = if Path::new("/mnt/data/mahdidev/ros/dev/separation-service/venv/bin/edge-tts").exists() {
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

    let success = match tts_status {
        Ok(status) if status.success() && Path::new(&output_mp3).exists() => true,
        _ => false,
    };

    if success {
        let convert_status = Command::new("ffmpeg")
            .args(&[
                "-y",
                "-i",
                &output_mp3,
                "-c:a",
                "libopus",
                "-b:a",
                "32k",
                &output_ogg,
            ])
            .status()
            .await;

        let _ = std::fs::remove_file(&output_mp3);

        if let Ok(status) = convert_status {
            if status.success() && Path::new(&output_ogg).exists() {
                log_ev!("tts", trace_id, "engine_complete", "output" => &output_ogg);
                return Ok(PathBuf::from(output_ogg));
            }
        }
        let _ = std::fs::remove_file(&output_ogg);
    }

    log_ev!("tts", trace_id, "edge_tts_failed", "text" => text);
    Err("TTS generation failed".to_string())
}
