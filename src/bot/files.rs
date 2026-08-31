use std::path::Path;

use frankenstein::{AsyncTelegramApi, client_reqwest::Bot, methods::GetFileParams};

use crate::log::next_trace_id;
use crate::youtube::trace::log_trace;

/// Ceiling for one `getFile`. Generous — a 2 GB upload takes minutes for the
/// local server to fetch — but bounded, so a stuck file fails instead of hanging.
const GET_FILE_TIMEOUT_SECS: u64 = 600;

/// Result of a Telegram file download with timing and byte count.
#[derive(Debug, Clone)]
pub struct TransferResult {
    pub bytes: u64,
    pub elapsed: std::time::Duration,
}

impl TransferResult {
    /// Speed in bytes per second.
    pub fn speed_bps(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs > 0.0 {
            self.bytes as f64 / secs
        } else {
            0.0
        }
    }

    /// Human-readable speed string (e.g. "12.3 MB/s").
    pub fn speed_human(&self) -> String {
        let bps = self.speed_bps();
        if bps >= 1_000_000.0 {
            format!("{:.1} MB/s", bps / 1_000_000.0)
        } else if bps >= 1_000.0 {
            format!("{:.0} KB/s", bps / 1_000.0)
        } else {
            format!("{:.0} B/s", bps)
        }
    }
}

/// Download a Telegram file (by `file_id`) to destination path `dest`.
/// Handles both Local Bot API (local disk copy with path validation) and HTTP download.
pub async fn download_telegram_file(
    api: &Bot,
    file_id: &str,
    dest: impl AsRef<Path>,
) -> Result<TransferResult, Box<dyn std::error::Error + Send + Sync>> {
    download_telegram_file_metered(api, file_id, dest, None, None).await
}

pub async fn download_telegram_file_metered(
    api: &Bot,
    file_id: &str,
    dest: impl AsRef<Path>,
    progress: Option<&std::sync::Arc<crate::bot::transfer::TransferProgress>>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<TransferResult, Box<dyn std::error::Error + Send + Sync>> {
    let dl_start = std::time::Instant::now();
    let dest = dest.as_ref();

    if let Some(p) = progress {
        p.set_stage(crate::bot::transfer::Stage::Fetching);
    }

    let file_info = tokio::time::timeout(
        std::time::Duration::from_secs(GET_FILE_TIMEOUT_SECS),
        api.get_file(&GetFileParams::builder().file_id(file_id).build()),
    )
    .await
    .map_err(|_| format!("getFile timed out after {GET_FILE_TIMEOUT_SECS}s"))??;
    let file_path = file_info.result.file_path.ok_or("no file_path")?;

    if let Some(p) = progress {
        p.set_total(file_info.result.file_size.unwrap_or(0) as u64);
    }

    let trace = next_trace_id();
    let path_label = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<file>");
    log_trace(trace, "download_file", &format!("file_name={path_label}"));

    if file_path.starts_with('/') {
        let allowed_prefix = std::env::var("TELEGRAM_LOCAL_STORAGE_DIR")
            .unwrap_or_else(|_| "/var/lib/telegram-bot-api".to_string());
        let allowed_canonical = std::path::Path::new(&allowed_prefix)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(&allowed_prefix));
        let canonical = std::path::Path::new(&file_path).canonicalize().ok();
        let is_safe = canonical.as_ref().map_or(false, |p| {
            p.starts_with(&allowed_prefix) || p.starts_with(&allowed_canonical)
        }) || file_path.starts_with(&allowed_prefix)
            || file_path.starts_with(allowed_canonical.to_str().unwrap_or(""));
        if !is_safe {
            return Err("file path outside allowed local directory".into());
        }

        if let Some(p) = progress {
            p.set_stage(crate::bot::transfer::Stage::Copying);
        }

        let mut f_in = tokio::fs::File::open(&file_path).await?;
        let mut f_out = tokio::fs::File::create(dest).await?;
        let mut bytes_copied = 0u64;
        let mut buf = vec![0u8; 1024 * 1024]; // 1MB buffer

        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            if let Some(c) = &cancel {
                if c.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
            }
            let n = f_in.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            f_out.write_all(&buf[..n]).await?;
            bytes_copied += n as u64;
            if let Some(p) = progress {
                p.bump(n as u64);
            }
        }

        let elapsed = dl_start.elapsed();
        log_trace(
            trace,
            "download_file_local_copy",
            &format!("size={bytes_copied} elapsed={:.2}s", elapsed.as_secs_f64()),
        );
        crate::bot::transfer::record_fetch_sample(bytes_copied, elapsed).await;
        if let Some(p) = progress {
            p.set_stage(crate::bot::transfer::Stage::Done);
        }
        return Ok(TransferResult {
            bytes: bytes_copied,
            elapsed,
        });
    }

    let token = crate::config::bot_token().map_err(|e| e.to_string())?;
    let url = if let Some(base) = crate::config::bot_api_base_url() {
        let base = base.trim_end_matches('/');
        format!("{base}/file/bot{token}/{file_path}")
    } else {
        format!("https://api.telegram.org/file/bot{token}/{file_path}")
    };

    let client = crate::http::client();
    let mut response = client.get(&url).send().await?;
    let mut file = tokio::fs::File::create(dest).await?;
    let mut bytes_copied = 0u64;

    if let Some(p) = progress {
        p.set_stage(crate::bot::transfer::Stage::Streaming);
    }

    while let Some(chunk) = response.chunk().await? {
        if let Some(c) = &cancel {
            if c.load(std::sync::atomic::Ordering::Relaxed) {
                return Err("cancelled".into());
            }
        }
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        bytes_copied += chunk.len() as u64;
        if let Some(p) = progress {
            p.bump(chunk.len() as u64);
        }
    }
    let elapsed = dl_start.elapsed();
    log_trace(
        trace,
        "download_file_http_done",
        &format!("bytes={bytes_copied} elapsed={:.2}s", elapsed.as_secs_f64()),
    );
    crate::bot::transfer::record_fetch_sample(bytes_copied, elapsed).await;
    if let Some(p) = progress {
        p.set_stage(crate::bot::transfer::Stage::Done);
    }
    Ok(TransferResult {
        bytes: bytes_copied,
        elapsed,
    })
}
