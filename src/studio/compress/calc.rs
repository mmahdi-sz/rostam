use std::path::Path;

use super::session::CompressSession;
use crate::i18n::{t, tf};

/// Calculates exact target output dimensions (width, height) preserving aspect ratio
/// and ensuring dimensions are even numbers for video encoder compatibility.
pub fn calculate_target_dimensions(orig_w: u32, orig_h: u32, target_scale: u32) -> (u32, u32) {
    let orig_w = orig_w.max(1);
    let orig_h = orig_h.max(1);
    let base_dim = orig_w.min(orig_h);
    if target_scale >= base_dim {
        return (orig_w, orig_h);
    }
    if orig_w >= orig_h {
        // Landscape or square: target_scale is height
        let target_h = target_scale;
        let mut target_w = ((orig_w as f64 * target_h as f64) / orig_h as f64).round() as u32;
        if !target_w.is_multiple_of(2) {
            target_w += 1;
        }
        (target_w.max(2), target_h.max(2))
    } else {
        // Portrait: target_scale is width
        let target_w = target_scale;
        let mut target_h = ((orig_h as f64 * target_w as f64) / orig_w as f64).round() as u32;
        if !target_h.is_multiple_of(2) {
            target_h += 1;
        }
        (target_w.max(2), target_h.max(2))
    }
}

/// Computes bitrate in kbps for the given resolution scale and ratio percentage.
pub fn calculate_target_bitrate_kbps(
    session: &CompressSession,
    target_scale: u32,
    ratio_percent: u32,
) -> u64 {
    let orig_w = session.orig_w.max(1) as f64;
    let orig_h = session.orig_h.max(1) as f64;
    let (target_w, target_h) =
        calculate_target_dimensions(session.orig_w, session.orig_h, target_scale);

    let orig_pixels = orig_w * orig_h;
    let target_pixels = (target_w as f64) * (target_h as f64);
    let pixel_ratio = (target_pixels / orig_pixels).min(1.0);

    let orig_kbps = (session.orig_bitrate as f64 / 1000.0).max(100.0);
    let base_target_kbps = orig_kbps * pixel_ratio;

    let final_kbps = base_target_kbps * (ratio_percent as f64 / 100.0);
    (final_kbps.round() as u64).max(50)
}

/// Computes estimated output file size in MB.
#[allow(dead_code)]
pub fn calculate_estimated_size_mb(
    session: &CompressSession,
    target_scale: u32,
    ratio_percent: u32,
) -> f64 {
    let bitrate_kbps = calculate_target_bitrate_kbps(session, target_scale, ratio_percent);
    let total_bits = (bitrate_kbps * 1000) as f64 * session.duration_secs as f64;
    (total_bits / 8.0) / (1024.0 * 1024.0)
}

pub fn format_eta_hms(secs: u64) -> String {
    let hours = secs / 3600;
    let rem = secs % 3600;
    let mins = rem / 60;
    let seconds = rem % 60;

    let mut parts = Vec::new();
    if hours > 0 {
        parts.push(tf(
            "studio.compress.eta_unit_hours",
            &[("n", &hours.to_string())],
        ));
    }
    if mins > 0 {
        parts.push(tf(
            "studio.compress.eta_unit_minutes",
            &[("n", &mins.to_string())],
        ));
    }
    if seconds > 0 || parts.is_empty() {
        parts.push(tf(
            "studio.compress.eta_unit_seconds",
            &[("n", &seconds.to_string())],
        ));
    }
    parts.join(&t("studio.compress.eta_join_and"))
}

pub fn compute_vmaf_score(
    output_file: &Path,
    input_file: &Path,
    orig_w: u32,
    orig_h: u32,
    threads_arg: &str,
) -> String {
    let vmaf_filter =
        format!("[0:v]scale={orig_w}:{orig_h}[dist];[1:v][dist]libvmaf=n_threads={threads_arg}");
    let mut cmd = std::process::Command::new(crate::config::ffmpeg_path());
    cmd.args([
        "-i",
        output_file.to_str().unwrap_or_default(),
        "-i",
        input_file.to_str().unwrap_or_default(),
        "-filter_complex",
        &vmaf_filter,
        "-f",
        "null",
        "-",
    ])
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::piped());

    let Ok(out) = cmd.output() else {
        return "N/A".to_string();
    };

    let stderr = String::from_utf8_lossy(&out.stderr);
    for line in stderr.lines() {
        if let Some(pos) = line.find("VMAF score:") {
            let rest = &line[pos + "VMAF score:".len()..];
            if let Ok(score) = rest.trim().parse::<f64>() {
                return format!("{score:.2}");
            }
        }
    }
    "N/A".to_string()
}
