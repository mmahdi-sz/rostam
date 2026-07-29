use std::path::Path;
use std::process::Command;
use std::time::Instant;

const DEEP_FILTER_BIN: &str = "files/runtime/deep-filter";
const DF_MODEL: &str = "files/models/deepfilter/DeepFilterNet3_onnx.tar.gz";

/// Runs deep-filter to denoise `input_wav` -> `output_wav`.
/// Returns the elapsed time in seconds.
pub fn denoise(input_wav: &str, output_wav: &str) -> anyhow::Result<f64> {
    let start = Instant::now();

    let out_dir = Path::new(output_wav).parent().unwrap_or(Path::new("out"));
    let in_name = Path::new(input_wav)
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid input wav path: no file name"))?
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("input wav file name is not valid UTF-8"))?;

    let out_dir_str = out_dir
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("output directory path is not valid UTF-8"))?;

    let status = Command::new(DEEP_FILTER_BIN)
        .args(["-m", DF_MODEL, "-o", out_dir_str, input_wav])
        .status()
        .map_err(|e| anyhow::anyhow!("failed to run deep-filter: {e}"))?;

    if !status.success() {
        anyhow::bail!("deep-filter exited with non-zero status");
    }

    let expected = out_dir.join(in_name);
    if expected.as_os_str() != output_wav {
        std::fs::rename(&expected, output_wav)?;
    }

    let elapsed = start.elapsed().as_secs_f64();
    Ok(elapsed)
}
