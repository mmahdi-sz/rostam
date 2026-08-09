//! Shared progress state for one compression job.
//!
//! The ticker, the download loop and the engine all write here, so the status
//! message can say which stage is actually running. Before this, every stage
//! rendered `fc.processing` at 0% — a user waiting on a slow download saw
//! "compressing 0%" and assumed 7z had hung.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// 0 = downloading the user's files, 1 = the archiver is running.
pub const STAGE_DOWNLOAD: u8 = 0;
pub const STAGE_COMPRESS: u8 = 1;

#[derive(Default)]
pub struct JobProgress {
    stage: AtomicU8,
    file_idx: AtomicU8,
    file_total: AtomicU8,
    /// 0–100, written by the archiver's stdout parser. Stays 0 for formats
    /// that report no progress (tar/zstd), which is what suppresses the ETA.
    percent: AtomicU8,
    /// Seconds the job had already spent when compression started, so the ETA
    /// divides by compression time, not by download time.
    compress_offset: AtomicU64,
}

impl JobProgress {
    pub fn new(file_total: usize) -> Self {
        let p = Self::default();
        p.file_total
            .store(file_total.min(u8::MAX as usize) as u8, Ordering::Relaxed);
        p
    }

    pub fn set_downloading(&self, idx: usize) {
        self.stage.store(STAGE_DOWNLOAD, Ordering::Relaxed);
        self.file_idx
            .store(idx.min(u8::MAX as usize) as u8, Ordering::Relaxed);
    }

    pub fn set_compressing(&self, elapsed_secs: u64) {
        self.compress_offset.store(elapsed_secs, Ordering::Relaxed);
        self.stage.store(STAGE_COMPRESS, Ordering::Relaxed);
    }

    pub fn set_percent(&self, pct: u8) {
        self.percent.store(pct.min(100), Ordering::Relaxed);
    }

    pub fn stage(&self) -> u8 {
        self.stage.load(Ordering::Relaxed)
    }

    pub fn file_idx(&self) -> u8 {
        self.file_idx.load(Ordering::Relaxed)
    }

    pub fn file_total(&self) -> u8 {
        self.file_total.load(Ordering::Relaxed)
    }

    pub fn percent(&self) -> u8 {
        self.percent.load(Ordering::Relaxed)
    }

    pub fn compress_offset(&self) -> u64 {
        self.compress_offset.load(Ordering::Relaxed)
    }
}

/// Ten-cell bar, filled by 10% steps.
pub fn bar(percent: u8) -> String {
    let filled = (percent.min(100) / 10) as usize;
    "█".repeat(filled) + &"░".repeat(10 - filled)
}

/// Remaining seconds extrapolated from the compression rate so far, or `None`
/// until there is enough signal (the first percent tick is noise).
pub fn eta_secs(percent: u8, compress_elapsed: u64) -> Option<u64> {
    if percent < 5 || percent >= 100 || compress_elapsed < 3 {
        return None;
    }
    let total = compress_elapsed as f64 * 100.0 / f64::from(percent);
    Some((total - compress_elapsed as f64).max(0.0).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bar_steps() {
        assert_eq!(bar(0), "░░░░░░░░░░");
        assert_eq!(bar(50), "█████░░░░░");
        assert_eq!(bar(100), "██████████");
        // Rounds down: 49% must not claim the fifth cell.
        assert_eq!(bar(49), "████░░░░░░");
    }

    #[test]
    fn test_eta_needs_signal() {
        // Too early, or already done — no ETA rather than a wrong one.
        assert_eq!(eta_secs(0, 10), None);
        assert_eq!(eta_secs(4, 10), None);
        assert_eq!(eta_secs(50, 2), None);
        assert_eq!(eta_secs(100, 60), None);
    }

    #[test]
    fn test_eta_extrapolates_linearly() {
        // 25% took 30s => 90s left.
        assert_eq!(eta_secs(25, 30), Some(90));
        assert_eq!(eta_secs(50, 60), Some(60));
    }

    #[test]
    fn test_progress_stage_transitions() {
        let p = JobProgress::new(3);
        assert_eq!(p.file_total(), 3);
        p.set_downloading(2);
        assert_eq!(p.stage(), STAGE_DOWNLOAD);
        assert_eq!(p.file_idx(), 2);
        p.set_compressing(42);
        assert_eq!(p.stage(), STAGE_COMPRESS);
        assert_eq!(p.compress_offset(), 42);
        p.set_percent(200);
        assert_eq!(p.percent(), 100);
    }
}
