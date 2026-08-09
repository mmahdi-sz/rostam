use std::path::Path;

use frankenstein::{AsyncTelegramApi, client_reqwest::Bot, methods::GetFileParams};

use crate::log::next_trace_id;
use crate::youtube::trace::log_trace;

/// Ceiling for one `getFile`. Generous — a 2 GB upload takes minutes for the
/// local server to fetch — but bounded, so a stuck file fails instead of hanging.
const GET_FILE_TIMEOUT_SECS: u64 = 600;

/// Download a Telegram file (by `file_id`) to destination path `dest`.
/// Handles both Local Bot API (local disk copy with path validation) and HTTP download.
pub async fn download_telegram_file(
    api: &Bot,
    file_id: &str,
    dest: impl AsRef<Path>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dest = dest.as_ref();

    // In `--local` mode the Bot API server pulls the whole file from Telegram
    // before answering getFile, and answers nothing until it does — an
    // unbounded wait that used to park the caller behind a silent status message.
    let file_info = tokio::time::timeout(
        std::time::Duration::from_secs(GET_FILE_TIMEOUT_SECS),
        api.get_file(&GetFileParams::builder().file_id(file_id).build()),
    )
    .await
    .map_err(|_| format!("getFile timed out after {GET_FILE_TIMEOUT_SECS}s"))??;
    let file_path = file_info.result.file_path.ok_or("no file_path")?;

    let trace = next_trace_id();
    let path_label = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<file>");
    log_trace(trace, "download_file", &format!("file_name={path_label}"));

    // Local Bot API returns an absolute filesystem path in --local mode.
    if file_path.starts_with('/') {
        let allowed_prefix = std::env::var("TELEGRAM_LOCAL_STORAGE_DIR")
            .unwrap_or_else(|_| "/var/lib/telegram-bot-api".to_string());
        let canonical = std::path::Path::new(&file_path).canonicalize().ok();
        let is_safe = canonical
            .as_ref()
            .map_or(false, |p| p.starts_with(&allowed_prefix))
            || file_path.starts_with(&allowed_prefix);
        if !is_safe {
            return Err("file path outside allowed local directory".into());
        }
        std::fs::copy(&file_path, dest)?;
        let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        log_trace(trace, "download_file_local_copy", &format!("size={size}"));
        return Ok(());
    }

    let token = crate::config::bot_token().map_err(|e| e.to_string())?;
    let url = if let Some(base) = crate::config::bot_api_base_url() {
        let base = base.trim_end_matches('/');
        format!("{base}/file/bot{token}/{file_path}")
    } else {
        format!("https://api.telegram.org/file/bot{token}/{file_path}")
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    let mut response = client.get(&url).send().await?;
    let mut file = tokio::fs::File::create(dest).await?;
    let mut bytes_copied = 0u64;
    while let Some(chunk) = response.chunk().await? {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        bytes_copied += chunk.len() as u64;
    }
    log_trace(
        trace,
        "download_file_http_done",
        &format!("bytes={bytes_copied}"),
    );
    Ok(())
}
