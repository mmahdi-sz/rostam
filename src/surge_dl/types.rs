use std::path::PathBuf;

pub const MAX_PART_BYTES: u64 = 2000 * 1024 * 1024;
pub const DOWNLOAD_TIMEOUT_SECS: u64 = 2 * 3600;
pub const POLL_INTERVAL_SECS: u64 = 3;

pub const CB_TOOLS_SURGE: &str = "tools:surge";
pub const CB_SURGE_CANCEL: &str = "surge:cancel";
pub const CB_SURGE_CONFIRM_ORIGINAL: &str = "surge:confirm:orig";
pub const CB_SURGE_CONFIRM_RENAME: &str = "surge:confirm:rename";

pub(crate) struct SurgeDetail {
    pub(crate) filename: String,
    pub(crate) url: String,
    pub(crate) total_size: u64,
    pub(crate) downloaded: u64,
    pub(crate) progress: f64,
    pub(crate) speed: f64,
    pub(crate) avg_speed: f64,
    pub(crate) status: String,
}

pub(crate) struct DirCleanupGuard(pub(crate) PathBuf);

impl Drop for DirCleanupGuard {
    fn drop(&mut self) {
        let path = self.0.clone();
        tokio::spawn(async move {
            let _ = tokio::fs::remove_dir_all(path).await;
        });
    }
}
