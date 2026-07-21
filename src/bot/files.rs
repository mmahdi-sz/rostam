use std::path::Path;

use frankenstein::{AsyncTelegramApi, client_reqwest::Bot, methods::GetFileParams};

use crate::log::next_trace_id;
use crate::youtube::trace::log_trace;

/// دانلود یک فایل تلگرام (با `file_id`) به مسیر مقصد `dest`.
///
/// این تابع مشترک جای ۶ کپیِ قبلیِ `download_file` در بخش‌های مختلف
/// (stt/denoise/upscale/gwm/pdfcompress/…) را می‌گیرد. دو رفتار مهم:
///
/// 1. **حالت Local Bot API**: وقتی ربات با سرور محلی Bot API کار می‌کند،
///    تلگرام به‌جای URL، مسیر مطلق فایل روی دیسک را برمی‌گرداند
///    (با `/` شروع می‌شود). در این حالت مستقیم از دیسک کپی می‌کنیم — اما
///    فقط اگر مسیر واقعاً داخل پوشه‌ی مجاز ذخیره‌سازی تلگرام باشد. این
///    چک امنیتی جلوی خواندن فایل‌های دلخواه سیستم (path traversal) را
///    می‌گیرد، به‌ویژه چون پروسه با کاربر root اجرا می‌شود.
///
/// 2. **حالت HTTP**: در غیر این صورت فایل را از طریق HTTP دانلود می‌کنیم.
///    توکن و آدرس پایه از `config` خوانده می‌شوند (نه مستقیم از env) تا
///    زنجیره‌ی `.env → /etc/default/abc → env` رعایت شود.
///
/// خطا از نوع `Send + Sync` است تا در همه‌ی فراخوان‌ها (از جمله داخل
/// `tokio::spawn`) قابل استفاده باشد.
pub async fn download_telegram_file(
    api: &Bot,
    file_id: &str,
    dest: impl AsRef<Path>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dest = dest.as_ref();

    let file_info = api
        .get_file(&GetFileParams::builder().file_id(file_id).build())
        .await?;
    let file_path = file_info.result.file_path.ok_or("no file_path")?;

    let trace = next_trace_id();
    log_trace(trace, "download_file", &format!("file_path={file_path}"));

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

    let mut response = reqwest::get(&url).await?;
    let mut file = tokio::fs::File::create(dest).await?;
    let mut bytes_copied = 0u64;
    while let Some(chunk) = response.chunk().await? {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        bytes_copied += chunk.len() as u64;
    }
    log_trace(trace, "download_file_http_done", &format!("bytes={bytes_copied}"));
    Ok(())
}
