//! Shared pipeline infrastructure for Photo & Video Magic Studio media editing tools.
//!
//! Provides RAII cleanup guards, active job cancellation registries, and shared
//! job progress tracking for future Studio tools (crop, watermark, format conversion, trim).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// RAII guard for temporary directories created during Studio operations.
/// Guarantees directory removal on drop across all exit paths (normal return, error, cancel, panic).
#[derive(Debug)]
pub struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Global registry of active Studio jobs, mapping `user_id -> cancel_flag`.
static ACTIVE_STUDIO_JOBS: LazyLock<Mutex<HashMap<i64, Arc<AtomicBool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Registers a cancel flag for a user's Studio job.
pub fn register_active_job(user_id: i64, cancel_flag: Arc<AtomicBool>) {
    if let Ok(mut jobs) = ACTIVE_STUDIO_JOBS.lock() {
        jobs.insert(user_id, cancel_flag);
    }
}

/// Removes and returns the cancel flag for a user's Studio job.
pub fn remove_active_job(user_id: i64) -> Option<Arc<AtomicBool>> {
    if let Ok(mut jobs) = ACTIVE_STUDIO_JOBS.lock() {
        jobs.remove(&user_id)
    } else {
        None
    }
}

/// Signals cancellation for a user's active Studio job.
pub fn cancel_active_job(user_id: i64) -> bool {
    if let Ok(mut jobs) = ACTIVE_STUDIO_JOBS.lock() {
        if let Some(flag) = jobs.remove(&user_id) {
            flag.store(true, Ordering::Relaxed);
            return true;
        }
    }
    false
}

/// Spawns a background task that periodically updates `status_msg_id` with live download stats
/// (elapsed time, downloaded size, total size, percentage, speed, and ETA) until `stop_flag` is set.
pub fn spawn_download_ticker(
    api: frankenstein::client_reqwest::Bot,
    chat_id: i64,
    status_msg_id: i32,
    dest_path: PathBuf,
    total_bytes: u64,
    domain_prefix: &'static str,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> Arc<AtomicBool> {
    use frankenstein::{AsyncTelegramApi, ParseMode, methods::EditMessageTextParams};
    use std::time::{Duration, Instant};
    use crate::i18n::{apply_premium_to_md, md_escape, tf};
    use crate::studio::compress::format_eta_hms;

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_inner = stop_flag.clone();
    let start_time = Instant::now();

    crate::app::spawn_user_task(async move {
        let mut last_rendered = String::new();

        while !stop_inner.load(Ordering::Relaxed) {
            if let Some(cf) = &cancel_flag {
                if cf.load(Ordering::Relaxed) {
                    break;
                }
            }

            let elapsed_secs = start_time.elapsed().as_secs();
            let downloaded_bytes = std::fs::metadata(&dest_path)
                .map(|m| m.len())
                .unwrap_or(0);

            let speed_bps = if elapsed_secs > 0 {
                downloaded_bytes as f64 / elapsed_secs as f64
            } else {
                0.0
            };

            let speed_str = if speed_bps >= 1_000_000.0 {
                format!("{:.1} MB/s", speed_bps / 1_000_000.0)
            } else if speed_bps >= 1_000.0 {
                format!("{:.0} KB/s", speed_bps / 1_000.0)
            } else {
                format!("{:.0} B/s", speed_bps)
            };

            let detail_key = format!("{domain_prefix}.status_downloading_detail");
            let main_key = format!("{domain_prefix}.status_downloading");

            let detail_param = if downloaded_bytes > 0 && total_bytes > 0 {
                let dl_mb = format!("{:.1}", downloaded_bytes as f64 / 1_048_576.0);
                let total_mb = format!("{:.1}", total_bytes as f64 / 1_048_576.0);
                let pct = (downloaded_bytes * 100 / total_bytes).min(100);

                let eta_secs = if speed_bps > 0.0 && total_bytes > downloaded_bytes {
                    ((total_bytes - downloaded_bytes) as f64 / speed_bps) as u64
                } else {
                    0
                };
                let eta_str = format_eta_hms(eta_secs);

                tf(
                    &detail_key,
                    &[
                        ("dl_mb", &md_escape(&dl_mb)),
                        ("total_mb", &md_escape(&total_mb)),
                        ("pct", &pct.to_string()),
                        ("speed", &md_escape(&speed_str)),
                        ("eta", &md_escape(&eta_str)),
                    ],
                )
            } else if speed_bps > 0.0 {
                let dl_mb = format!("{:.1}", downloaded_bytes as f64 / 1_048_576.0);
                let speed_esc = md_escape(&speed_str);
                format!("\n📥 *دانلودشده:* `{dl_mb} مگابایت`\n🚀 *سرعت:* `{speed_esc}`")
            } else {
                String::new()
            };

            let elapsed_str = format!("{elapsed_secs}s");
            let text_key = format!("{downloaded_bytes}:{elapsed_secs}");

            if text_key != last_rendered {
                last_rendered = text_key;
                let raw_ticker = tf(
                    &main_key,
                    &[
                        ("elapsed", &md_escape(&elapsed_str)),
                        ("detail", &detail_param),
                    ],
                );
                let text = apply_premium_to_md(&raw_ticker);

                let builder = EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .text(&text)
                    .parse_mode(ParseMode::MarkdownV2);

                let params = if domain_prefix == "studio.trim" {
                    builder.reply_markup(crate::studio::trim::job_cancel_keyboard()).build()
                } else {
                    builder.build()
                };

                let _ = api.edit_message_text(&params).await;
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    stop_flag
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temp_dir_guard_removes_dir() {
        let temp = std::env::temp_dir().join(format!("studio_test_guard_{}", rand_id()));
        std::fs::create_dir_all(&temp).unwrap();
        assert!(temp.exists());

        {
            let _guard = TempDirGuard::new(temp.clone());
        }

        assert!(!temp.exists());
    }

    #[test]
    fn test_active_job_registry() {
        let uid = 987654321;
        let flag = Arc::new(AtomicBool::new(false));
        register_active_job(uid, flag.clone());

        assert!(!flag.load(Ordering::Relaxed));
        let cancelled = cancel_active_job(uid);
        assert!(cancelled);
        assert!(flag.load(Ordering::Relaxed));
        assert!(remove_active_job(uid).is_none());
    }

    fn rand_id() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(12345)
    }
}
