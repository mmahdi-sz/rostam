use std::process::Command;

#[derive(Debug)]
pub enum GwmError {
    IoError(String),
    NoWatermarkDetected(String), // exit 1 + "[SKIP]" in stdout
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

/// Strip ANSI escape sequences so we can safely search plain-text keywords.
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

fn remove_sync(image_bytes: &[u8], ext: &str, user_id: i64, trace_id: u64) -> Result<Vec<u8>, GwmError> {
    let tmp_dir = std::env::temp_dir().join(format!("gwm_{trace_id}_{user_id}"));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| GwmError::IoError(e.to_string()))?;

    let input_path = tmp_dir.join(format!("input.{ext}"));
    let output_path = tmp_dir.join(format!("output.{ext}"));

    std::fs::write(&input_path, image_bytes).map_err(|e| GwmError::IoError(e.to_string()))?;

    eprintln!(
        "[gwm trace={trace_id} event=binary_run] binary={GWT_BINARY} \
         input={} output={} user_id={user_id}",
        input_path.display(), output_path.display()
    );

    let t_start = std::time::Instant::now();
    let result = Command::new(GWT_BINARY)
        .arg("-i").arg(&input_path)
        .arg("-o").arg(&output_path)
        .arg("--quiet")
        .arg("--no-banner")
        .output()
        .map_err(|e| {
            std::fs::remove_dir_all(&tmp_dir).ok();
            GwmError::IoError(format!("failed to spawn gwt-mini: {e}"))
        })?;

    let elapsed = t_start.elapsed().as_secs_f64();
    let exit_code = result.status.code().unwrap_or(-1);
    let raw_stdout = String::from_utf8_lossy(&result.stdout);
    let raw_stderr = String::from_utf8_lossy(&result.stderr);
    let stdout_plain = strip_ansi(&raw_stdout);
    let stderr_plain = strip_ansi(&raw_stderr);

    eprintln!(
        "[gwm trace={trace_id} event=binary_exit] exit_code={exit_code} elapsed={elapsed:.2}s \
         stdout={stdout_plain:?} stderr={stderr_plain:?}"
    );

    if !result.status.success() {
        std::fs::remove_dir_all(&tmp_dir).ok();

        // exit 1 with [SKIP] in stdout → no watermark detected, not a hard failure
        let stdout_lower = stdout_plain.to_lowercase();
        if exit_code == 1
            && (stdout_lower.contains("[skip]")
                || stdout_lower.contains("no watermark detected")
                || stdout_lower.contains("skipped"))
        {
            let detail = stdout_plain.lines()
                .find(|l| l.to_lowercase().contains("skip") || l.to_lowercase().contains("watermark"))
                .unwrap_or("no watermark detected")
                .trim()
                .to_string();
            eprintln!("[gwm trace={trace_id} event=no_watermark] detail={detail:?}");
            return Err(GwmError::NoWatermarkDetected(detail));
        }

        return Err(GwmError::BinaryFailed(format!(
            "exit code {exit_code} — stdout: {stdout_plain} stderr: {stderr_plain}"
        )));
    }

    if !output_path.exists() {
        std::fs::remove_dir_all(&tmp_dir).ok();
        eprintln!("[gwm trace={trace_id} event=no_output_file]");
        return Err(GwmError::BinaryFailed("gwt-mini exited 0 but produced no output file".into()));
    }

    let output_bytes = std::fs::read(&output_path).map_err(|e| GwmError::IoError(e.to_string()))?;
    std::fs::remove_dir_all(&tmp_dir).ok();

    eprintln!(
        "[gwm trace={trace_id} event=output_ready] bytes={} elapsed={elapsed:.2}s",
        output_bytes.len()
    );
    Ok(output_bytes)
}
