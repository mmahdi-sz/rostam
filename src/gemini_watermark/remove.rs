use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[derive(Debug)]
pub enum GwmError {
    IoError(String),
    NoWatermarkDetected(String), // both attempts returned [SKIP]
    BinaryFailed(String),
}

impl std::fmt::Display for GwmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "IO error: {msg}"),
            Self::NoWatermarkDetected(msg) => write!(f, "no watermark: {msg}"),
            Self::BinaryFailed(msg) => write!(f, "gwt-mini failed: {msg}"),
        }
    }
}

const GWT_BINARY: &str = "files/runtime/gwt-mini";

/// Inpainting method we feed to gwt-mini after the alpha-map removal.
/// TELEA is fast (no GPU), produces a clean result, and applied at radius=20
/// covers JPEG ringing around the watermark area.
const DENOISE_METHOD: &str = "telea";
const INPAINT_RADIUS: &str = "20";

pub async fn remove_watermark(
    image_bytes: Vec<u8>,
    ext: String,
    user_id: i64,
    trace_id: u64,
) -> Result<Vec<u8>, GwmError> {
    tokio::task::spawn_blocking(move || remove_sync(&image_bytes, &ext, user_id, trace_id))
        .await
        .map_err(|e| GwmError::IoError(format!("spawn_blocking: {e}")))?
}

/// Strip ANSI escape sequences so we can safely search plain-text keywords
/// in stdout/stderr produced by gwt-mini's colorized output.
fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\x1b' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2;
            while i < bytes.len() && !bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            i += 1; // skip final letter
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

#[derive(Debug)]
struct GwtRun {
    exit_code: i32,
    success: bool,
    stdout: String,
    stderr: String,
    elapsed_secs: f64,
    skipped: bool,
}

fn run_gwt(input: &Path, output: &Path, extra_args: &[&str], trace_id: u64, attempt: u32) -> Result<GwtRun, GwmError> {
    let mut cmd = Command::new(GWT_BINARY);
    cmd.arg("-i").arg(input)
        .arg("-o").arg(output)
        .arg("--denoise").arg(DENOISE_METHOD)
        .arg("--radius").arg(INPAINT_RADIUS)
        .arg("--quiet")
        .arg("--no-banner");
    for a in extra_args {
        cmd.arg(a);
    }

    let args_dbg: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
    eprintln!(
        "[gwm trace={trace_id} event=binary_spawn] attempt={attempt} binary={GWT_BINARY} \
         args={args_dbg:?}"
    );

    let t0 = Instant::now();
    let result = cmd.output().map_err(|e| GwmError::IoError(format!("failed to spawn gwt-mini: {e}")))?;
    let elapsed_secs = t0.elapsed().as_secs_f64();

    let exit_code = result.status.code().unwrap_or(-1);
    let stdout = strip_ansi(&String::from_utf8_lossy(&result.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&result.stderr));
    let stdout_lower = stdout.to_lowercase();
    let skipped = stdout_lower.contains("[skip]")
        || stdout_lower.contains("no watermark detected")
        || stdout_lower.contains("skipped");

    // ALWAYS log the full stdout/stderr (not truncated) so we can diagnose
    // any future failure without re-running.
    eprintln!(
        "[gwm trace={trace_id} event=binary_exit] attempt={attempt} exit_code={exit_code} \
         success={success} skipped={skipped} elapsed={elapsed_secs:.2}s",
        success = result.status.success(),
    );
    eprintln!("[gwm trace={trace_id} event=binary_stdout] attempt={attempt} stdout={stdout:?}");
    eprintln!("[gwm trace={trace_id} event=binary_stderr] attempt={attempt} stderr={stderr:?}");

    Ok(GwtRun {
        exit_code,
        success: result.status.success(),
        stdout,
        stderr,
        elapsed_secs,
        skipped,
    })
}

fn remove_sync(image_bytes: &[u8], ext: &str, user_id: i64, trace_id: u64) -> Result<Vec<u8>, GwmError> {
    let tmp_dir = std::env::temp_dir().join(format!("gwm_{trace_id}_{user_id}"));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| GwmError::IoError(e.to_string()))?;

    let input_path = tmp_dir.join(format!("input.{ext}"));
    let output_path = tmp_dir.join(format!("output.{ext}"));

    std::fs::write(&input_path, image_bytes).map_err(|e| GwmError::IoError(e.to_string()))?;
    eprintln!(
        "[gwm trace={trace_id} event=workdir_ready] user_id={user_id} workdir={:?} input_bytes={}",
        tmp_dir, image_bytes.len()
    );

    let total_start = Instant::now();

    // Attempt 1: current profile + TELEA denoise + radius 20.
    // For modern Gemini outputs this is the right path.
    let r1 = match run_gwt(&input_path, &output_path, &[], trace_id, 1) {
        Ok(r) => r,
        Err(e) => {
            std::fs::remove_dir_all(&tmp_dir).ok();
            return Err(e);
        }
    };

    if r1.success && !r1.skipped && output_path.exists() {
        return finalize_success(&tmp_dir, &output_path, trace_id, total_start.elapsed().as_secs_f64(), "current");
    }

    // gwt-mini v0.3.1 BUG: when --denoise is set, the automatic current→legacy
    // fallback is silently disabled. If current profile said "[SKIP]", retry
    // explicitly pinned to the legacy profile. This handles pre-Gemini-3.5 outputs.
    if r1.skipped || (!r1.success && r1.exit_code == 1) {
        eprintln!("[gwm trace={trace_id} event=fallback_to_legacy] reason=attempt1_skipped");
        // Clean stale output (if any) so we can re-check existence cleanly.
        let _ = std::fs::remove_file(&output_path);

        let r2 = match run_gwt(&input_path, &output_path, &["--legacy"], trace_id, 2) {
            Ok(r) => r,
            Err(e) => {
                std::fs::remove_dir_all(&tmp_dir).ok();
                return Err(e);
            }
        };

        if r2.success && !r2.skipped && output_path.exists() {
            return finalize_success(&tmp_dir, &output_path, trace_id, total_start.elapsed().as_secs_f64(), "legacy");
        }

        // Both attempts skipped → genuinely no watermark.
        if r2.skipped || (r2.exit_code == 1 && !r2.success) {
            std::fs::remove_dir_all(&tmp_dir).ok();
            let detail = pick_summary(&r2.stdout)
                .or_else(|| pick_summary(&r1.stdout))
                .unwrap_or_else(|| "no watermark detected in either profile".to_string());
            eprintln!(
                "[gwm trace={trace_id} event=no_watermark_final] \
                 attempt1_exit={} attempt2_exit={} detail={detail:?}",
                r1.exit_code, r2.exit_code,
            );
            return Err(GwmError::NoWatermarkDetected(detail));
        }

        std::fs::remove_dir_all(&tmp_dir).ok();
        return Err(GwmError::BinaryFailed(format!(
            "attempt2 (legacy) exit={} stdout={:?} stderr={:?}",
            r2.exit_code, r2.stdout, r2.stderr,
        )));
    }

    // Attempt 1 failed for some other reason.
    std::fs::remove_dir_all(&tmp_dir).ok();
    Err(GwmError::BinaryFailed(format!(
        "attempt1 exit={} stdout={:?} stderr={:?}",
        r1.exit_code, r1.stdout, r1.stderr,
    )))
}

fn finalize_success(
    tmp_dir: &PathBuf,
    output_path: &PathBuf,
    trace_id: u64,
    total_elapsed: f64,
    profile_used: &str,
) -> Result<Vec<u8>, GwmError> {
    let output_bytes = std::fs::read(output_path).map_err(|e| GwmError::IoError(e.to_string()))?;
    std::fs::remove_dir_all(tmp_dir).ok();
    eprintln!(
        "[gwm trace={trace_id} event=output_ready] profile={profile_used} \
         output_bytes={} total_elapsed={total_elapsed:.2}s",
        output_bytes.len(),
    );
    Ok(output_bytes)
}

fn pick_summary(stdout: &str) -> Option<String> {
    stdout.lines()
        .find(|l| {
            let lo = l.to_lowercase();
            lo.contains("skip") || lo.contains("watermark") || lo.contains("detect")
        })
        .map(|l| l.trim().to_string())
}
