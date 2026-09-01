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
    if !(5..100).contains(&percent) || compress_elapsed < 3 {
        return None;
    }
    let total = compress_elapsed as f64 * 100.0 / f64::from(percent);
    Some((total - compress_elapsed as f64).max(0.0).round() as u64)
}

use frankenstein::types::InlineKeyboardMarkup;

use super::config::{CompressAlgo, CompressConfig, CompressFmt};
use crate::emoji::panel::{
    btn_icon, btn_icon_danger, btn_icon_plain, btn_icon_primary, btn_icon_success,
};
use crate::i18n::t;

pub const CB_FC_CANCEL: &str = "fc:cancel";

// ── Keyboards ──────────────────────────────────────────────────────────────────

pub fn options_keyboard(config: &CompressConfig) -> InlineKeyboardMarkup {
    let mut rows = Vec::new();

    // Row 1: Format selection (ZIP / 7Z / RAR)
    let zip_btn = if config.fmt == CompressFmt::Zip {
        btn_icon_success("ZIP", "fc:fmt:zip", "pack_folder")
    } else {
        btn_icon_plain("ZIP", "fc:fmt:zip", "pack_folder")
    };
    let sz_btn = if config.fmt == CompressFmt::SevenZ {
        btn_icon_success("7Z", "fc:fmt:7z", "7zip_logo")
    } else {
        btn_icon_plain("7Z", "fc:fmt:7z", "7zip_logo")
    };
    let rar_btn = if config.fmt == CompressFmt::Rar {
        btn_icon_success("RAR", "fc:fmt:rar", "rar_logo")
    } else {
        btn_icon_plain("RAR", "fc:fmt:rar", "rar_logo")
    };
    // zstd icon not set yet — icon name will be filled later
    let zstd_btn = if config.fmt == CompressFmt::Zstd {
        btn_icon_success("ZSTD", "fc:fmt:zstd", "")
    } else {
        btn_icon_plain("ZSTD", "fc:fmt:zstd", "")
    };
    rows.push(vec![zip_btn, sz_btn, rar_btn, zstd_btn]);

    // Row 2: Algorithm selection (7Z only)
    if config.fmt == CompressFmt::SevenZ {
        let lzma2_btn = if config.algo == CompressAlgo::Lzma2 {
            btn_icon_success("LZMA2", "fc:algo:lzma2", "sparkles")
        } else {
            btn_icon_plain("LZMA2", "fc:algo:lzma2", "")
        };
        let ppmd_btn = if config.algo == CompressAlgo::Ppmd {
            btn_icon_success("PPMd", "fc:algo:ppmd", "sparkles")
        } else {
            btn_icon_plain("PPMd", "fc:algo:ppmd", "")
        };
        let bzip2_btn = if config.algo == CompressAlgo::Bzip2 {
            btn_icon_success("BZip2", "fc:algo:bzip2", "sparkles")
        } else {
            btn_icon_plain("BZip2", "fc:algo:bzip2", "")
        };
        rows.push(vec![lzma2_btn, ppmd_btn, bzip2_btn]);
    }

    // Row 3: Compression Level Title (BLUE per user request!)
    let level_text = if config.level == 0 {
        t("fc.level_label_store")
    } else {
        t("fc.level_label").replace("{level}", &config.level.to_string())
    };
    rows.push(vec![btn_icon(&level_text, "fc:noop", "panel")]);

    // Row 4: Compression Level Controls (- and +)
    rows.push(vec![
        btn_icon("\u{200B}", "fc:lvl:down", "prev"),
        btn_icon("\u{200B}", "fc:lvl:up", "next"),
    ]);

    // Row 5: Password Encryption Toggle — Formats without password support do not get button.
    if config.fmt.supports_password() {
        let (pass_label, pass_btn) = if config.password.is_some() {
            (
                t("fc.toggle_password"),
                btn_icon_success(&t("fc.status_on"), "fc:toggle:pass", "check"),
            )
        } else {
            (
                t("fc.toggle_password"),
                btn_icon_danger(&t("fc.status_off"), "fc:toggle:pass", "cross"),
            )
        };
        rows.push(vec![
            btn_icon_plain(&pass_label, "fc:toggle:pass", "warning"),
            pass_btn,
        ]);
    }

    // Row 6: Split into parts Toggle
    if config.fmt.supports_split() {
        let (split_label, split_btn) = if let Some(mb) = config.split_mb {
            (
                t("fc.toggle_split"),
                btn_icon_success(&format!("{mb} MB"), "fc:toggle:split", "check"),
            )
        } else {
            (
                t("fc.toggle_split"),
                btn_icon_danger(&t("fc.status_off"), "fc:toggle:split", "cross"),
            )
        };
        rows.push(vec![
            btn_icon_plain(&split_label, "fc:toggle:split", "replace_mode"),
            split_btn,
        ]);
    }

    // Split size controls if split enabled
    if let Some(mb) = config.split_mb {
        rows.push(vec![btn_icon_plain(
            &t("fc.part_size_label").replace("{mb}", &mb.to_string()),
            "fc:noop",
            "info",
        )]);
        rows.push(vec![
            btn_icon_plain("+5", "fc:part:+5", ""),
            btn_icon_plain("+10", "fc:part:+10", ""),
            btn_icon_plain("+25", "fc:part:+25", ""),
            btn_icon_plain("+50", "fc:part:+50", ""),
            btn_icon_plain("+100", "fc:part:+100", ""),
            btn_icon_plain("+250", "fc:part:+250", ""),
        ]);
        rows.push(vec![
            btn_icon_plain("-5", "fc:part:-5", ""),
            btn_icon_plain("-10", "fc:part:-10", ""),
            btn_icon_plain("-25", "fc:part:-25", ""),
            btn_icon_plain("-50", "fc:part:-50", ""),
            btn_icon_plain("-100", "fc:part:-100", ""),
            btn_icon_plain("-250", "fc:part:-250", ""),
        ]);
    }

    // Row 8 (7Z only): Header Encryption (Obfuscate) Toggle
    if config.fmt == CompressFmt::SevenZ {
        let (obf_label, obf_btn) = if config.obfuscate {
            (
                t("fc.toggle_obfuscate"),
                btn_icon_success(&t("fc.status_on"), "fc:toggle:obfuscate", "check"),
            )
        } else {
            (
                t("fc.toggle_obfuscate"),
                btn_icon_danger(&t("fc.status_off"), "fc:toggle:obfuscate", "cross"),
            )
        };
        rows.push(vec![
            btn_icon_plain(&obf_label, "fc:toggle:obfuscate", "eye"),
            obf_btn,
        ]);
    }

    // Row 9: Solid Mode Toggle
    // tar.zst is always a single stream, so solid mode is not selectable.
    if config.fmt.supports_solid() {
        let solid_btn = if config.solid {
            btn_icon_primary(&t("fc.solid_mode_solid"), "fc:toggle:solid", "pack_folder")
        } else {
            btn_icon_success(&t("fc.solid_mode_normal"), "fc:toggle:solid", "rocket")
        };
        rows.push(vec![
            btn_icon(&t("fc.toggle_solid"), "fc:toggle:solid", "pack_folder"),
            solid_btn,
        ]);
    }

    // Row 10: Confirm + Cancel
    rows.push(vec![
        btn_icon_success(&t("fc.confirm_button"), "fc:confirm", "confirm"),
        btn_icon_plain(&t("start.back"), CB_FC_CANCEL, "back"),
    ]);

    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

/// Cancel button only — for password prompt step.
pub fn cancel_only_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("fc.cancel_button"),
            CB_FC_CANCEL,
            "cancel",
        )]])
        .build()
}

/// Cancel button on progress message — cancels active job.
pub fn job_cancel_keyboard() -> InlineKeyboardMarkup {
    crate::common::job_cancel_keyboard(&t("fc.cancel_button"), "fc:jobcancel", "cancel")
}

pub fn done_inline_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![
            btn_icon_success(&t("fc.done_upload_button"), "fc:done", "confirm"),
            btn_icon_danger(&t("fc.cancel_button"), CB_FC_CANCEL, "cancel"),
        ]])
        .build()
}

/// Renders the status message for whichever stage is running. `elapsed` is the
/// whole job's wall time; the ETA uses only the compression part of it.
pub fn render_progress(progress: &JobProgress, elapsed: u64) -> String {
    if progress.stage() == STAGE_DOWNLOAD {
        return t("fc.downloading")
            .replace("{idx}", &progress.file_idx().to_string())
            .replace("{total}", &progress.file_total().to_string())
            .replace("{elapsed}", &format_clock(elapsed));
    }
    let pct = progress.percent();
    let compress_elapsed = elapsed.saturating_sub(progress.compress_offset());
    match eta_secs(pct, compress_elapsed) {
        Some(eta) => t("fc.processing_eta")
            .replace("{bar}", &bar(pct))
            .replace("{percent}", &pct.to_string())
            .replace("{elapsed}", &format_clock(elapsed))
            .replace("{eta}", &format_clock(eta)),
        None => t("fc.processing")
            .replace("{bar}", &bar(pct))
            .replace("{percent}", &pct.to_string())
            .replace("{elapsed}", &format_clock(elapsed)),
    }
}

/// mm:ss (or hh:mm:ss) for elapsed time display.
pub fn format_clock(secs: u64) -> String {
    if secs >= 3600 {
        format!(
            "{:02}:{:02}:{:02}",
            secs / 3600,
            (secs % 3600) / 60,
            secs % 60
        )
    } else {
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}

// Test-only access to real keyboards and formatting
#[cfg(feature = "testapi")]
pub fn options_keyboard_for_test(config: &CompressConfig) -> InlineKeyboardMarkup {
    options_keyboard(config)
}

#[cfg(feature = "testapi")]
pub fn cancel_only_keyboard_for_test() -> InlineKeyboardMarkup {
    cancel_only_keyboard()
}

#[cfg(feature = "testapi")]
pub fn job_cancel_keyboard_for_test() -> InlineKeyboardMarkup {
    job_cancel_keyboard()
}

#[cfg(feature = "testapi")]
pub fn render_progress_for_test(progress: &JobProgress, elapsed: u64) -> String {
    render_progress(progress, elapsed)
}

#[cfg(feature = "testapi")]
pub fn format_clock_for_test(secs: u64) -> String {
    format_clock(secs)
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
