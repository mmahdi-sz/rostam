use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[derive(Debug)]
pub enum GwmError {
    IoError(String),
    NoWatermarkDetected(String), // pass 1 returned [SKIP] on both current AND legacy profiles
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

/// One successfully-completed pass. Returned in order from pass 1 to the
/// final pass that produced output. Callers may show all of them to the user
/// so they can pick the trade-off they prefer (earlier = more detail
/// preserved, later = cleaner residual).
#[derive(Debug)]
pub struct PassOutput {
    pub pass_num: u32,
    pub confidence_note: Option<String>,
    pub bytes: Vec<u8>,
}

const GWT_BINARY: &str = "files/runtime/gwt-mini";

/// Cleanup method after alpha-map removal. TELEA inpainting fills the residual
/// without GPU overhead and gives a perceptually clean result.
const DENOISE_METHOD: &str = "telea";
/// Maximum inpaint radius the binary accepts (hard-coded ceiling: 1-25).
const INPAINT_RADIUS: &str = "25";
/// Lowered threshold for refinement passes (default is 0.25). Pass 1 uses the
/// default to act as a true "is there a watermark" gate; passes 2+ accept much
/// weaker residual signals because we already know a watermark existed.
const REFINEMENT_THRESHOLD: &str = "0.05";
/// Total passes attempted (pass 1 = detection, passes 2+ = residual cleanup).
/// Empirically pass 4 always fails because the binary's spatial confidence
/// goes negative once the watermark area looks more like background.
const MAX_PASSES: u32 = 3;

pub async fn remove_watermark(
    image_bytes: Vec<u8>,
    ext: String,
    user_id: i64,
    trace_id: u64,
) -> Result<Vec<PassOutput>, GwmError> {
    tokio::task::spawn_blocking(move || remove_sync(&image_bytes, &ext, user_id, trace_id))
        .await
        .map_err(|e| GwmError::IoError(format!("spawn_blocking: {e}")))?
}

/// Strip ANSI escape sequences so we can match plain-text keywords in stdout/stderr.
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
            i += 1;
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

fn run_gwt(
    input: &Path,
    output: &Path,
    extra_args: &[&str],
    trace_id: u64,
    pass_num: u32,
    profile: &str,
) -> Result<GwtRun, GwmError> {
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
        "[gwm trace={trace_id} event=binary_spawn] pass={pass_num} profile={profile} \
         binary={GWT_BINARY} args={args_dbg:?}"
    );

    let t0 = Instant::now();
    let result = cmd
        .output()
        .map_err(|e| GwmError::IoError(format!("failed to spawn gwt-mini: {e}")))?;
    let elapsed_secs = t0.elapsed().as_secs_f64();

    let exit_code = result.status.code().unwrap_or(-1);
    let stdout = strip_ansi(&String::from_utf8_lossy(&result.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&result.stderr));
    let stdout_lower = stdout.to_lowercase();
    let skipped = stdout_lower.contains("[skip]")
        || stdout_lower.contains("no watermark detected")
        || stdout_lower.contains("skipped");

    eprintln!(
        "[gwm trace={trace_id} event=binary_exit] pass={pass_num} profile={profile} \
         exit_code={exit_code} success={success} skipped={skipped} elapsed={elapsed_secs:.2}s",
        success = result.status.success(),
    );
    eprintln!(
        "[gwm trace={trace_id} event=binary_stdout] pass={pass_num} profile={profile} stdout={stdout:?}"
    );
    eprintln!(
        "[gwm trace={trace_id} event=binary_stderr] pass={pass_num} profile={profile} stderr={stderr:?}"
    );

    Ok(GwtRun { exit_code, success: result.status.success(), stdout, stderr, elapsed_secs, skipped })
}

fn pick_summary(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find(|l| {
            let lo = l.to_lowercase();
            lo.contains("skip") || lo.contains("watermark") || lo.contains("detect")
        })
        .map(|l| l.trim().to_string())
}

fn remove_sync(
    image_bytes: &[u8],
    ext: &str,
    user_id: i64,
    trace_id: u64,
) -> Result<Vec<PassOutput>, GwmError> {
    let work_dir = std::env::temp_dir().join(format!("gwm_{trace_id}_{user_id}"));
    std::fs::create_dir_all(&work_dir).map_err(|e| GwmError::IoError(e.to_string()))?;

    let pass0_path: PathBuf = work_dir.join(format!("pass0.{ext}"));
    std::fs::write(&pass0_path, image_bytes).map_err(|e| GwmError::IoError(e.to_string()))?;

    eprintln!(
        "[gwm trace={trace_id} event=multi_pass_start] user_id={user_id} workdir={:?} \
         input_bytes={} max_passes={MAX_PASSES} radius={INPAINT_RADIUS} denoise={DENOISE_METHOD}",
        work_dir, image_bytes.len()
    );
    let total_start = Instant::now();

    // Each entry is a completed pass we want to expose to the user.
    let mut completed: Vec<(u32, PathBuf, Option<String>)> = Vec::new();

    // ─── Pass 1: detection with current profile, then legacy fallback ───
    let pass1_path: PathBuf = work_dir.join(format!("pass1.{ext}"));

    let r1_current = run_gwt(&pass0_path, &pass1_path, &[], trace_id, 1, "current")?;

    let profile_used: &'static str;
    let profile_args: &'static [&'static str];

    if r1_current.success && !r1_current.skipped && pass1_path.exists() {
        profile_used = "current";
        profile_args = &[];
        completed.push((1, pass1_path.clone(), pick_summary(&r1_current.stdout)));
    } else if r1_current.skipped {
        eprintln!("[gwm trace={trace_id} event=fallback_to_legacy] reason=current_skipped");
        let _ = std::fs::remove_file(&pass1_path);

        let r1_legacy = run_gwt(&pass0_path, &pass1_path, &["--legacy"], trace_id, 1, "legacy")?;

        if r1_legacy.success && !r1_legacy.skipped && pass1_path.exists() {
            profile_used = "legacy";
            profile_args = &["--legacy"];
            completed.push((1, pass1_path.clone(), pick_summary(&r1_legacy.stdout)));
        } else if r1_legacy.skipped {
            std::fs::remove_dir_all(&work_dir).ok();
            let detail = pick_summary(&r1_legacy.stdout)
                .or_else(|| pick_summary(&r1_current.stdout))
                .unwrap_or_else(|| "no watermark detected in either profile".to_string());
            eprintln!(
                "[gwm trace={trace_id} event=no_watermark_final] \
                 attempt_current_exit={} attempt_legacy_exit={} detail={detail:?}",
                r1_current.exit_code, r1_legacy.exit_code,
            );
            return Err(GwmError::NoWatermarkDetected(detail));
        } else {
            std::fs::remove_dir_all(&work_dir).ok();
            return Err(GwmError::BinaryFailed(format!(
                "pass1 legacy exit={} stdout={:?} stderr={:?}",
                r1_legacy.exit_code, r1_legacy.stdout, r1_legacy.stderr,
            )));
        }
    } else {
        std::fs::remove_dir_all(&work_dir).ok();
        return Err(GwmError::BinaryFailed(format!(
            "pass1 current exit={} stdout={:?} stderr={:?}",
            r1_current.exit_code, r1_current.stdout, r1_current.stderr,
        )));
    }

    eprintln!(
        "[gwm trace={trace_id} event=pass_complete] pass=1 profile={profile_used} \
         output={:?}",
        pass1_path
    );

    // ─── Passes 2..=MAX_PASSES: residual cleanup with lowered threshold ───
    let mut current_input = pass1_path;

    for pass_num in 2..=MAX_PASSES {
        let next_path: PathBuf = work_dir.join(format!("pass{pass_num}.{ext}"));
        let mut args: Vec<&str> = profile_args.to_vec();
        args.push("--threshold");
        args.push(REFINEMENT_THRESHOLD);

        let r = match run_gwt(&current_input, &next_path, &args, trace_id, pass_num, profile_used) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "[gwm trace={trace_id} event=pass_spawn_failed] pass={pass_num} err={e} \
                     — keeping {} pass(es) so far",
                    completed.len()
                );
                break;
            }
        };

        if r.skipped {
            eprintln!(
                "[gwm trace={trace_id} event=pass_no_residual] pass={pass_num} \
                 — watermark fully removed after {} pass(es)",
                completed.len()
            );
            break;
        }

        if !r.success || !next_path.exists() {
            eprintln!(
                "[gwm trace={trace_id} event=pass_no_output] pass={pass_num} exit_code={} \
                 — keeping {} pass(es) so far",
                r.exit_code,
                completed.len()
            );
            break;
        }

        eprintln!(
            "[gwm trace={trace_id} event=pass_complete] pass={pass_num} profile={profile_used} \
             output={:?}",
            next_path
        );
        completed.push((pass_num, next_path.clone(), pick_summary(&r.stdout)));
        current_input = next_path;
    }

    // Read bytes for each completed pass before deleting the work dir.
    let mut outputs: Vec<PassOutput> = Vec::with_capacity(completed.len());
    for (pass_num, path, note) in &completed {
        let bytes = std::fs::read(path).map_err(|e| GwmError::IoError(e.to_string()))?;
        outputs.push(PassOutput {
            pass_num: *pass_num,
            confidence_note: note.clone(),
            bytes,
        });
    }

    std::fs::remove_dir_all(&work_dir).ok();

    let total_elapsed = total_start.elapsed().as_secs_f64();
    eprintln!(
        "[gwm trace={trace_id} event=multi_pass_done] profile={profile_used} \
         passes_completed={} total_elapsed={total_elapsed:.2}s",
        outputs.len()
    );
    for o in &outputs {
        eprintln!(
            "[gwm trace={trace_id} event=output_summary] pass={} bytes={} note={:?}",
            o.pass_num,
            o.bytes.len(),
            o.confidence_note,
        );
    }

    Ok(outputs)
}
