use std::process::Command;

#[derive(Debug)]
pub enum GwmError {
    IoError(String),
    BinaryFailed(String),
}

impl std::fmt::Display for GwmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "IO error: {msg}"),
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

fn remove_sync(image_bytes: &[u8], ext: &str, user_id: i64, trace_id: u64) -> Result<Vec<u8>, GwmError> {
    let tmp_dir = std::env::temp_dir().join(format!("gwm_{trace_id}_{user_id}"));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| GwmError::IoError(e.to_string()))?;

    let input_path = tmp_dir.join(format!("input.{ext}"));
    let output_path = tmp_dir.join(format!("output.{ext}"));

    std::fs::write(&input_path, image_bytes).map_err(|e| GwmError::IoError(e.to_string()))?;

    eprintln!(
        "[gwm trace={trace_id} event=binary_run] binary={GWT_BINARY} input={} output={}",
        input_path.display(), output_path.display()
    );

    let result = Command::new(GWT_BINARY)
        .arg("-i").arg(&input_path)
        .arg("-o").arg(&output_path)
        .arg("--quiet")
        .arg("--no-banner")
        .output()
        .map_err(|e| GwmError::IoError(format!("failed to run gwt-mini: {e}")))?;

    let stderr = String::from_utf8_lossy(&result.stderr);
    let stdout = String::from_utf8_lossy(&result.stdout);
    eprintln!(
        "[gwm trace={trace_id} event=binary_done] status={} stdout={:?} stderr={:?}",
        result.status,
        &stdout.chars().take(200).collect::<String>(),
        &stderr.chars().take(200).collect::<String>(),
    );

    if !result.status.success() {
        std::fs::remove_dir_all(&tmp_dir).ok();
        return Err(GwmError::BinaryFailed(format!(
            "exit code {}: {stderr}",
            result.status.code().unwrap_or(-1)
        )));
    }

    if !output_path.exists() {
        std::fs::remove_dir_all(&tmp_dir).ok();
        return Err(GwmError::BinaryFailed("gwt-mini did not produce output file".into()));
    }

    let output_bytes = std::fs::read(&output_path).map_err(|e| GwmError::IoError(e.to_string()))?;
    std::fs::remove_dir_all(&tmp_dir).ok();

    eprintln!("[gwm trace={trace_id} event=output_read] bytes={}", output_bytes.len());
    Ok(output_bytes)
}
