use std::time::Instant;
use std::process::Command;
use std::path::Path;

const DEEP_FILTER_BIN: &str = "files/runtime/deep-filter";
const DF_MODEL: &str = "files/models/deepfilter/DeepFilterNet3_onnx.tar.gz";

/// Runs deep-filter to denoise `input_wav` -> `output_wav`.
/// Returns the elapsed time in seconds.
pub fn denoise(input_wav: &str, output_wav: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let start = Instant::now();

    let out_dir = Path::new(output_wav).parent().unwrap_or(Path::new("out"));
    let in_name = Path::new(input_wav).file_name().unwrap().to_str().unwrap();

    let status = Command::new(DEEP_FILTER_BIN)
        .args([
            "-m", DF_MODEL,
            "-o", out_dir.to_str().unwrap(),
            input_wav,
        ])
        .status()
        .map_err(|e| format!("failed to run deep-filter: {e}"))?;

    if !status.success() {
        return Err("deep-filter exited with non-zero status".into());
    }

    let expected = out_dir.join(in_name);
    if expected.as_os_str() != output_wav {
        std::fs::rename(&expected, output_wav)?;
    }

    let elapsed = start.elapsed().as_secs_f64();
    Ok(elapsed)
}
