//! Session state management, work directory setup, cancel handles, and keyboards.

use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use frankenstein::types::InlineKeyboardMarkup;

use super::subtitle::SubtitleFormat;
use crate::emoji::panel::btn_icon_danger;
use crate::i18n::t;
use crate::studio::pipeline::{register_active_job, remove_active_job};

pub fn cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("studio.burn.cancel_btn"),
            crate::bot::constants::CB_STUDIO_BURN_CANCEL,
            "cancel",
        )]])
        .build()
}

pub fn job_cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("studio.burn.cancel_btn"),
            crate::bot::constants::CB_STUDIO_BURN_JOBCANCEL,
            "cancel",
        )]])
        .build()
}

#[derive(Debug, Clone)]
pub struct VideoInputInfo {
    /// Sanitized name used only for display/caption, never as a path component.
    pub display_name: String,
    pub total_bytes: u64,
    pub local_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SubtitleInputInfo {
    pub format: SubtitleFormat,
    pub local_path: PathBuf,
}

#[derive(Debug)]
pub struct BurnSession {
    pub user_id: i64,
    pub chat_id: i64,
    pub status_msg_id: i32,
    pub work_dir: PathBuf,
    pub video_info: Option<VideoInputInfo>,
    pub subtitle_info: Option<SubtitleInputInfo>,
    pub cancel_flag: Arc<AtomicBool>,
    pub dl_stop_flag: Option<Arc<AtomicBool>>,
    /// Set once the video download finished successfully.
    pub video_ready: bool,
    /// Set once the burn job has been handed off, so it can never start twice.
    pub job_started: bool,
}

/// Claims the right to start the burn job atomically under the session mutex. Both ingest paths call it; exactly one wins.
pub fn try_claim_job(session: &Arc<Mutex<BurnSession>>) -> bool {
    let Ok(mut s) = session.lock() else {
        return false;
    };
    if s.video_ready && s.subtitle_info.is_some() && !s.job_started {
        s.job_started = true;
        return true;
    }
    false
}

pub fn stop_download_ticker(session: &Arc<Mutex<BurnSession>>) {
    if let Ok(mut s) = session.lock() {
        if let Some(flag) = s.dl_stop_flag.take() {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

/// Tears a session down: stops the ticker, signals cancel, drops the registry entry and work dir.
pub fn abort_session(session: &Arc<Mutex<BurnSession>>) {
    stop_download_ticker(session);
    let (user_id, work_dir) = {
        let Ok(s) = session.lock() else { return };
        s.cancel_flag.store(true, Ordering::Relaxed);
        (s.user_id, s.work_dir.clone())
    };
    remove_active_job(user_id);
    if work_dir.exists() {
        let _ = std::fs::remove_dir_all(&work_dir);
    }
}

/// Creates the work dir plus a registered session for a fresh burn flow.
pub fn new_session(
    user_id: i64,
    chat_id: i64,
    status_msg_id: i32,
    trace_id: u64,
) -> Option<Arc<Mutex<BurnSession>>> {
    let work_dir = std::env::temp_dir().join(format!("studio_burn_{trace_id}_{user_id}"));
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        log_ev!("studio_burn", trace_id, "mkdir_failed", "=>" => format!("fail err={e}"));
        return None;
    }

    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_active_job(user_id, cancel_flag.clone());

    Some(Arc::new(Mutex::new(BurnSession {
        user_id,
        chat_id,
        status_msg_id,
        work_dir,
        video_info: None,
        subtitle_info: None,
        cancel_flag,
        dl_stop_flag: None,
        video_ready: false,
        job_started: false,
    })))
}
