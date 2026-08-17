//! Hardsub subtitle burning module for Photo & Video Magic Studio (`studio_burn`).
//!
//! Handles subtitle detection (SRT, ASS/SSA, WebVTT), ASS native style preservation vs SRT/VTT
//! force_style, background video ingestion with a live download ticker, brokered ffmpeg execution
//! with `-progress pipe:1`, cancel-aware waiting, and re-arming back to the burn prompt.
//!
//! Inputs are copied to fixed in-work-dir names (`input.<ext>`, `sub.<ext>`) so neither the
//! filesystem path nor the ffmpeg filtergraph ever carries a user-controlled string.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{DeleteMessageParams, EditMessageTextParams, SendMessageParams, SendVideoParams},
    types::{InlineKeyboardMarkup, Message, ReplyMarkup},
};

use crate::bot::transfer::send_file_with_upload_ticker;
use crate::bot::{files::download_telegram_file, send_text_md};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState, panel::btn_icon_danger};
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::log::next_trace_id;
use crate::moebius::cpu::{
    acquire_cpu, is_user_cpu_busy, pin_current_thread, release_cpu, trim_memory,
};
use crate::rank::{effective_rank, paywall::block_feature, types::Rank};
use crate::stats::{record_error_global, record_event_global, record_event_user};
use crate::validation::sanitize_filename;

use super::compress::format_eta_hms;
use super::pipeline::{
    TempDirGuard, register_active_job, remove_active_job, spawn_download_ticker,
};

/// Hardsub re-encode is CPU-bound; anything longer would hold a broker slot for hours.
pub const MAX_BURN_DURATION_SECS: u64 = 7200;
/// Telegram Bot API upload ceiling.
pub const MAX_UPLOAD_BYTES: u64 = 2000 * 1024 * 1024;
/// Minimum gap between burn ticker edits (Telegram rejects faster edit rates).
const TICKER_MIN_INTERVAL: Duration = Duration::from_secs(3);

pub const DEFAULT_SUBTITLE_FONT: &str = "Arial";
pub const DEFAULT_SUBTITLE_FONTSIZE: u32 = 18;
pub const DEFAULT_SUBTITLE_PRIMARY_COLOR: &str = "&H00FFFFFF";
pub const DEFAULT_SUBTITLE_OUTLINE_COLOR: &str = "&H00000000";
pub const DEFAULT_SUBTITLE_BORDER_STYLE: u32 = 1;
pub const DEFAULT_SUBTITLE_OUTLINE: u32 = 2;
pub const DEFAULT_SUBTITLE_SHADOW: u32 = 1;
pub const DEFAULT_SUBTITLE_ALIGNMENT: u32 = 2;

pub fn build_force_style_arg() -> String {
    format!(
        "Fontname={DEFAULT_SUBTITLE_FONT},Fontsize={DEFAULT_SUBTITLE_FONTSIZE},\
         PrimaryColour={DEFAULT_SUBTITLE_PRIMARY_COLOR},OutlineColour={DEFAULT_SUBTITLE_OUTLINE_COLOR},\
         BorderStyle={DEFAULT_SUBTITLE_BORDER_STYLE},Outline={DEFAULT_SUBTITLE_OUTLINE},\
         Shadow={DEFAULT_SUBTITLE_SHADOW},Alignment={DEFAULT_SUBTITLE_ALIGNMENT}"
    )
}

/// Escapes one value for an *unquoted* ffmpeg filtergraph argument. ffmpeg parses the
/// filtergraph itself, so shell quoting rules do not apply: every character that would
/// terminate or split the argument takes a single backslash.
pub fn escape_filter_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '\\' | '\'' | ':' | ',' | ';' | '=' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub fn escape_ffmpeg_filter_path(path: &Path) -> String {
    escape_filter_value(&path.to_string_lossy())
}

/// Builds the `-vf` argument: ASS keeps its own styling, SRT/VTT get the forced default style.
pub fn build_filter_arg(format: SubtitleFormat, sub_path: &Path) -> String {
    let path = escape_ffmpeg_filter_path(sub_path);
    match format {
        SubtitleFormat::Ass => format!("ass=filename={path}"),
        SubtitleFormat::Srt | SubtitleFormat::Vtt => format!(
            "subtitles=filename={path}:force_style={}",
            escape_filter_value(&build_force_style_arg())
        ),
    }
}

/// Video encoder args matched to the source codec. Hardsub always re-encodes, and re-encoding an
/// AV1/HEVC source with libx264 inflates the file 2-3x at the same visual quality — a 900 MB AV1
/// input came back over the 2000 MB upload cap. CRF values are per-encoder scales, each roughly
/// equivalent to x264 CRF 22. Unknown/absent codec falls back to x264 (widest Telegram support).
/// ponytail: no 10-bit handling — pix_fmt is only forced on the x264 path, elsewhere ffmpeg keeps
/// the source format.
pub fn video_encoder_args(source_codec: &str) -> Vec<&'static str> {
    match source_codec.trim().to_ascii_lowercase().as_str() {
        "av1" => vec!["-c:v", "libsvtav1", "-preset", "9", "-crf", "32"],
        "hevc" | "h265" => vec!["-c:v", "libx265", "-preset", "medium", "-crf", "26"],
        "vp9" => vec![
            "-c:v",
            "libvpx-vp9",
            "-crf",
            "32",
            "-b:v",
            "0",
            "-row-mt",
            "1",
        ],
        _ => vec![
            "-c:v", "libx264", "-preset", "medium", "-crf", "22", "-pix_fmt", "yuv420p",
        ],
    }
}

/// How many pieces an oversized output must be cut into to fit under the upload cap. Always ≥2 —
/// this is only called once the output is known to be over the cap.
pub fn upload_part_count(output_bytes: u64, cap_bytes: u64) -> u64 {
    if cap_bytes == 0 {
        return 2;
    }
    output_bytes.div_ceil(cap_bytes).max(2)
}

/// Segment length that cuts `total_duration` into `parts` roughly equal pieces.
pub fn split_segment_secs(total_duration: u64, parts: u64) -> u64 {
    (total_duration / parts.max(1)).max(1)
}

/// Splits a finished video into `parts` roughly equal pieces by duration. Stream-copies, so there
/// is no second re-encode. Segment cuts land on keyframes, so part sizes are only approximate —
/// the caller must still check every part against the upload cap.
/// ponytail: remux only (I/O bound, no filters), so it stays off the CPU broker.
fn split_video_into_parts(
    ffmpeg_bin: &Path,
    input: &Path,
    work_dir: &Path,
    total_duration: u64,
    parts: u64,
) -> Result<Vec<PathBuf>, String> {
    let segment_secs = split_segment_secs(total_duration, parts);
    let pattern = work_dir.join("part_%02d.mp4");

    let out = std::process::Command::new(ffmpeg_bin)
        .args(["-y", "-hide_banner", "-nostdin", "-i"])
        .arg(input)
        .args([
            "-c",
            "copy",
            "-map",
            "0",
            "-f",
            "segment",
            "-segment_time",
            &segment_secs.to_string(),
            "-reset_timestamps",
            "1",
        ])
        .arg(&pattern)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("ffmpeg segment spawn failed: {e}"))?;

    if !out.status.success() {
        let tail = String::from_utf8_lossy(&out.stderr);
        let tail: String = tail
            .chars()
            .rev()
            .take(400)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(format!("ffmpeg segment failed: {tail}"));
    }

    let mut found: Vec<PathBuf> = std::fs::read_dir(work_dir)
        .map_err(|e| format!("read work dir failed: {e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("part_") && n.ends_with(".mp4"))
        })
        .collect();
    found.sort();

    if found.is_empty() {
        return Err("segment produced no parts".to_string());
    }
    Ok(found)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    Srt,
    Ass,
    Vtt,
}

impl SubtitleFormat {
    /// Extension used for the copy stored inside the work dir.
    pub fn ext(&self) -> &'static str {
        match self {
            SubtitleFormat::Srt => "srt",
            SubtitleFormat::Ass => "ass",
            SubtitleFormat::Vtt => "vtt",
        }
    }
}

pub fn detect_subtitle_format(filename: &str) -> Option<SubtitleFormat> {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "srt" => Some(SubtitleFormat::Srt),
        "ass" | "ssa" => Some(SubtitleFormat::Ass),
        "vtt" => Some(SubtitleFormat::Vtt),
        _ => None,
    }
}

pub fn convert_vtt_to_srt(vtt_path: &Path, srt_path: &Path) -> anyhow::Result<()> {
    let output = std::process::Command::new(crate::config::ffmpeg_path())
        .args(["-y", "-hide_banner", "-nostdin", "-i"])
        .arg(vtt_path)
        .arg(srt_path)
        .output()
        .map_err(|e| anyhow::anyhow!("failed to execute ffmpeg for vtt conversion: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("vtt conversion failed: {err}");
    }
    Ok(())
}

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

/// Claims the right to start the burn job. Both ingest paths call it; exactly one wins.
fn try_claim_job(session: &Arc<Mutex<BurnSession>>) -> bool {
    let Ok(mut s) = session.lock() else {
        return false;
    };
    if s.video_ready && s.subtitle_info.is_some() && !s.job_started {
        s.job_started = true;
        return true;
    }
    false
}

fn stop_download_ticker(session: &Arc<Mutex<BurnSession>>) {
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
fn new_session(
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

/// Enters the Hardsub Burn prompt, setting `FlowState::AwaitingStudioBurnInput`.
pub async fn enter_burn_prompt(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    log_actor_id!("studio_burn", trace_id, user_id, "clicked" => crate::bot::constants::CB_STUDIO_BURN);

    // Re-entering the prompt abandons whatever was half-uploaded before.
    if let FlowState::AwaitingStudioBurnInput { session } = flow_manager.get(user_id) {
        log_ev!("studio_burn", trace_id, "abort_previous_session", "user_id" => user_id);
        abort_session(&session);
        flow_manager.clear(user_id);
    }

    // Fail closed: without a DB the rank cannot be verified, so the feature stays locked.
    let Some(db) = database else {
        log_ev!("studio_burn", trace_id, "rank_check_unavailable", "=>" => "blocked");
        record_error_global("studio_burn", "rank check unavailable: no database").await;
        let _ = send_text_md(api, chat_id, &t("studio.burn.error.burn_failed")).await;
        return;
    };

    let rank = effective_rank(db.client(), user_id).await;
    if rank.weight() < Rank::Esfandyar.weight() {
        log_ev!("studio_burn", trace_id, "paywall_blocked", "rank" => rank.as_str());
        record_event_user(user_id, "studio_burn", "paywall", "blocked", 1).await;
        record_event_global("studio_burn", "paywall", "blocked", 1).await;
        block_feature(
            api,
            chat_id,
            &t("studio.burn.feature_name"),
            Rank::Esfandyar,
        )
        .await;
        return;
    }

    if is_user_cpu_busy(user_id).await {
        log_ev!("studio_burn", trace_id, "cpu_busy", "=>" => "blocked");
        let _ = send_text_md(api, chat_id, &t("active_job_running")).await;
        return;
    }

    let Some(session) = new_session(user_id, chat_id, message_id, trace_id) else {
        record_error_global("studio_burn", "work dir creation failed").await;
        let _ = send_text_md(api, chat_id, &t("studio.burn.error.burn_failed")).await;
        return;
    };

    flow_manager.set(user_id, FlowState::AwaitingStudioBurnInput { session });

    let text = apply_premium_to_md(&t("studio.burn.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(cancel_keyboard())
        .build();

    let _ = api.edit_message_text(&params).await;
}

/// Re-arms the burn flow after a job ends: same prompt, same settings, as a new message.
async fn rearm_burn_prompt(api: &Bot, chat_id: i64, user_id: i64, flow_manager: &FlowManager) {
    let trace_id = next_trace_id();
    log_ev!("studio_burn", trace_id, "rearm", "user_id" => user_id);

    let text = apply_premium_to_md(&t("studio.burn.prompt"));
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(cancel_keyboard()))
        .build();

    let sent = match api.send_message(&params).await {
        Ok(msg) => msg.result,
        Err(e) => {
            log_ev!("studio_burn", trace_id, "rearm_failed", "=>" => format!("fail err={e}"));
            flow_manager.clear(user_id);
            return;
        }
    };

    match new_session(user_id, chat_id, sent.message_id, trace_id) {
        Some(session) => flow_manager.set(user_id, FlowState::AwaitingStudioBurnInput { session }),
        None => flow_manager.clear(user_id),
    }
}

/// Deletes the status message, reports `text`, drops the job entry and re-arms the flow.
async fn finish_with_error(
    api: &Bot,
    flow_manager: &FlowManager,
    chat_id: i64,
    user_id: i64,
    status_msg_id: i32,
    text: &str,
) {
    let _ = api
        .delete_message(
            &DeleteMessageParams::builder()
                .chat_id(chat_id)
                .message_id(status_msg_id)
                .build(),
        )
        .await;
    let _ = send_text_md(api, chat_id, text).await;
    remove_active_job(user_id);
    rearm_burn_prompt(api, chat_id, user_id, flow_manager).await;
}

/// Handles incoming video/document updates when flow state is `AwaitingStudioBurnInput`.
///
/// Subtitles are matched **first**: `is_video_message_metadata` treats unknown document mime
/// types as video, and Telegram sends `.srt` as `application/x-subrip`, so checking the video
/// branch first swallowed every subtitle upload.
pub async fn handle_input_message(
    api: &Bot,
    message: &Message,
    user_id: i64,
    session: Arc<Mutex<BurnSession>>,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;

    let sub_format = message
        .document
        .as_ref()
        .and_then(|doc| doc.file_name.as_deref().and_then(detect_subtitle_format));

    if let Some(fmt) = sub_format {
        handle_subtitle_input(
            api,
            message,
            user_id,
            fmt,
            session,
            flow_manager,
            database,
            trace_id,
        )
        .await;
        return;
    }

    if super::is_video_message_metadata(message) {
        handle_video_input(
            api,
            message,
            user_id,
            session,
            flow_manager,
            database,
            trace_id,
        )
        .await;
        return;
    }

    log_ev!("studio_burn", trace_id, "unsupported_input", "user_id" => user_id);
    let _ = send_text_md(api, chat_id, &t("studio.burn.error.unsupported_sub")).await;
}

#[allow(clippy::too_many_arguments)]
async fn handle_video_input(
    api: &Bot,
    message: &Message,
    user_id: i64,
    session: Arc<Mutex<BurnSession>>,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
    trace_id: u64,
) {
    log_actor_id!("studio_burn", trace_id, user_id, "uploaded" => "video");
    let chat_id = message.chat.id;

    let file_id = message
        .video
        .as_ref()
        .map(|v| v.file_id.clone())
        .or_else(|| message.document.as_ref().map(|d| d.file_id.clone()))
        .unwrap_or_default();
    if file_id.is_empty() {
        log_ev!("studio_burn", trace_id, "video_missing_file_id", "=>" => "fail");
        let _ = send_text_md(api, chat_id, &t("studio.burn.error.burn_failed")).await;
        return;
    }

    let raw_name = message
        .video
        .as_ref()
        .and_then(|v| v.file_name.as_deref())
        .or_else(|| {
            message
                .document
                .as_ref()
                .and_then(|d| d.file_name.as_deref())
        })
        .unwrap_or("video.mp4");
    let display_name = {
        let cleaned = sanitize_filename(raw_name);
        if cleaned.is_empty() {
            "video.mp4".to_string()
        } else {
            cleaned
        }
    };

    let total_bytes = message
        .video
        .as_ref()
        .and_then(|v| v.file_size)
        .or_else(|| message.document.as_ref().and_then(|d| d.file_size))
        .unwrap_or(0);

    // First video wins: replacing one mid-download would leave two tickers on one message.
    let (status_msg_id, local_path) = {
        let Ok(mut s) = session.lock() else { return };
        if s.video_info.is_some() {
            log_ev!("studio_burn", trace_id, "duplicate_video_ignored", "user_id" => user_id);
            return;
        }
        let ext = Path::new(&display_name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mp4")
            .to_lowercase();
        let local_path = s.work_dir.join(format!("input.{ext}"));

        let dl_stop = spawn_download_ticker(
            api.clone(),
            chat_id,
            s.status_msg_id,
            local_path.clone(),
            total_bytes,
            "studio.burn",
            Some(s.cancel_flag.clone()),
        );
        s.dl_stop_flag = Some(dl_stop);
        s.video_info = Some(VideoInputInfo {
            display_name,
            total_bytes,
            local_path: local_path.clone(),
        });
        (s.status_msg_id, local_path)
    };

    let api_cl = api.clone();
    let session_cl = session.clone();
    let db_cl = database.clone();
    let fm_cl = flow_manager.clone();

    crate::app::spawn_user_task(async move {
        let dl_res = download_telegram_file(&api_cl, &file_id, &local_path).await;
        stop_download_ticker(&session_cl);

        let cancelled = session_cl
            .lock()
            .map(|s| s.cancel_flag.load(Ordering::Relaxed))
            .unwrap_or(true);
        if cancelled {
            log_ev!("studio_burn", trace_id, "download_cancelled", "user_id" => user_id);
            return;
        }

        if let Err(e) = dl_res {
            log_ev!("studio_burn", trace_id, "video_download_failed", "=>" => format!("fail err={e}"));
            record_error_global("studio_burn", format!("video download failed: {e}")).await;
            record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
            let text = apply_premium_to_md(&t("studio.burn.error.download_failed"));
            abort_session(&session_cl);
            finish_with_error(&api_cl, &fm_cl, chat_id, user_id, status_msg_id, &text).await;
            return;
        }

        if let Ok(mut s) = session_cl.lock() {
            s.video_ready = true;
        }
        log_ev!("studio_burn", trace_id, "video_ready", "user_id" => user_id);

        if try_claim_job(&session_cl) {
            fm_cl.clear(user_id);
            execute_burn_job(&api_cl, session_cl, db_cl, fm_cl).await;
        } else {
            let text = apply_premium_to_md(&t("studio.burn.video_received_need_sub"));
            let params = EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(status_msg_id)
                .text(&text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(cancel_keyboard())
                .build();
            let _ = api_cl.edit_message_text(&params).await;
        }
    });
}

#[allow(clippy::too_many_arguments)]
async fn handle_subtitle_input(
    api: &Bot,
    message: &Message,
    user_id: i64,
    fmt: SubtitleFormat,
    session: Arc<Mutex<BurnSession>>,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
    trace_id: u64,
) {
    log_actor_id!("studio_burn", trace_id, user_id, "uploaded" => "subtitle");
    let chat_id = message.chat.id;
    let Some(doc) = message.document.as_ref() else {
        return;
    };

    let (work_dir, status_msg_id) = {
        let Ok(s) = session.lock() else { return };
        (s.work_dir.clone(), s.status_msg_id)
    };
    let sub_path = work_dir.join(format!("sub.{}", fmt.ext()));

    if let Err(e) = download_telegram_file(api, &doc.file_id, &sub_path).await {
        log_ev!("studio_burn", trace_id, "sub_download_failed", "=>" => format!("fail err={e}"));
        record_error_global("studio_burn", format!("subtitle download failed: {e}")).await;
        let _ = send_text_md(api, chat_id, &t("studio.burn.error.download_failed")).await;
        return;
    }

    let replaced = {
        let Ok(mut s) = session.lock() else { return };
        let replaced = s.subtitle_info.is_some();
        s.subtitle_info = Some(SubtitleInputInfo {
            format: fmt,
            local_path: sub_path,
        });
        replaced
    };

    if try_claim_job(&session) {
        flow_manager.clear(user_id);
        let api_cl = api.clone();
        let db_cl = database.clone();
        let fm_cl = flow_manager.clone();
        crate::app::spawn_user_task(async move {
            execute_burn_job(&api_cl, session, db_cl, fm_cl).await;
        });
        return;
    }

    // Video absent, or still downloading — its waiter starts the job when it lands.
    let raw_text = if replaced {
        t("studio.burn.sub_replaced")
    } else {
        t("studio.burn.sub_received_need_video")
    };
    let video_downloading = session
        .lock()
        .map(|s| s.video_info.is_some() && !s.video_ready)
        .unwrap_or(false);
    if video_downloading {
        log_ev!("studio_burn", trace_id, "sub_stored_awaiting_download", "user_id" => user_id);
        return;
    }

    let text = apply_premium_to_md(&raw_text);
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(status_msg_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(cancel_keyboard())
        .build();
    let _ = api.edit_message_text(&params).await;
}

/// Executes the brokered ffmpeg burn: probe, cap, CPU broker, live ticker, upload, re-arm.
pub async fn execute_burn_job(
    api: &Bot,
    session: Arc<Mutex<BurnSession>>,
    _database: Option<PostgresDatabase>,
    flow_manager: FlowManager,
) {
    let (user_id, chat_id, status_msg_id, work_dir, video_info, sub_info, cancel_flag) = {
        let Ok(s) = session.lock() else { return };
        (
            s.user_id,
            s.chat_id,
            s.status_msg_id,
            s.work_dir.clone(),
            s.video_info.clone(),
            s.subtitle_info.clone(),
            s.cancel_flag.clone(),
        )
    };
    stop_download_ticker(&session);

    let _guard = TempDirGuard::new(work_dir.clone());
    let trace_id = next_trace_id();
    log_ev!("studio_burn", trace_id, "execute_start", "user_id" => user_id);

    macro_rules! fail {
        ($key:expr) => {{
            let text = apply_premium_to_md(&t($key));
            finish_with_error(api, &flow_manager, chat_id, user_id, status_msg_id, &text).await;
        }};
    }

    let (Some(v_info), Some(s_info)) = (video_info, sub_info) else {
        log_ev!("studio_burn", trace_id, "missing_inputs", "=>" => "fail");
        record_error_global("studio_burn", "job started without both inputs").await;
        fail!("studio.burn.error.burn_failed");
        return;
    };

    if cancel_flag.load(Ordering::Relaxed) {
        log_ev!("studio_burn", trace_id, "cancelled", "stage" => "pre_probe");
        record_event_user(user_id, "studio_burn", "burn", "cancelled", 1).await;
        fail!("studio.burn.job_cancelled");
        return;
    }

    let video_bytes = std::fs::metadata(&v_info.local_path)
        .map(|m| m.len())
        .unwrap_or(0);
    if video_bytes == 0 {
        log_ev!("studio_burn", trace_id, "video_missing", "expected_bytes" => v_info.total_bytes, "=>" => "fail");
        record_error_global("studio_burn", "downloaded video missing or empty").await;
        record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
        fail!("studio.burn.error.download_failed");
        return;
    }

    let meta = match super::trim::run_ffprobe(&v_info.local_path) {
        Ok(m) => m,
        Err(e) => {
            log_ev!("studio_burn", trace_id, "ffprobe_failed", "=>" => format!("fail err={e}"));
            record_error_global("studio_burn", format!("ffprobe failed: {e}")).await;
            record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
            fail!("studio.burn.error.burn_failed");
            return;
        }
    };

    if meta.duration_secs > MAX_BURN_DURATION_SECS {
        log_ev!("studio_burn", trace_id, "too_long", "duration" => meta.duration_secs);
        record_event_user(user_id, "studio_burn", "burn", "too_long", 1).await;
        let text = apply_premium_to_md(&tf(
            "studio.burn.error.too_long",
            &[
                ("duration", &md_escape(&format_eta_hms(meta.duration_secs))),
                ("max", &md_escape(&format_eta_hms(MAX_BURN_DURATION_SECS))),
            ],
        ));
        finish_with_error(api, &flow_manager, chat_id, user_id, status_msg_id, &text).await;
        return;
    }

    // Checked here, not at prompt entry: minutes of uploading pass in between.
    if is_user_cpu_busy(user_id).await {
        log_ev!("studio_burn", trace_id, "cpu_busy", "=>" => "blocked");
        fail!("active_job_running");
        return;
    }

    let final_sub_path = if s_info.format == SubtitleFormat::Vtt {
        let converted = work_dir.join("sub_converted.srt");
        if let Err(e) = convert_vtt_to_srt(&s_info.local_path, &converted) {
            log_ev!("studio_burn", trace_id, "vtt_convert_failed", "=>" => format!("fail err={e}"));
            record_error_global("studio_burn", format!("vtt conversion failed: {e}")).await;
            record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
            fail!("studio.burn.error.unsupported_sub");
            return;
        }
        converted
    } else {
        s_info.local_path.clone()
    };

    let cores = acquire_cpu(user_id, trace_id).await;
    let threads_arg = if cores.is_empty() {
        "2".to_string()
    } else {
        cores.len().to_string()
    };

    // A user who cancels while queued must not lose the slot to a job that then runs anyway.
    if cancel_flag.load(Ordering::Relaxed) {
        release_cpu(cores, trace_id).await;
        trim_memory();
        log_ev!("studio_burn", trace_id, "cancelled", "stage" => "post_acquire");
        record_event_user(user_id, "studio_burn", "burn", "cancelled", 1).await;
        fail!("studio.burn.job_cancelled");
        return;
    }

    let filter_arg = build_filter_arg(s_info.format, &final_sub_path);
    let output_path = work_dir.join("output.mp4");
    let log_path = work_dir.join("ffmpeg.log");
    let display_stem = Path::new(&v_info.display_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("video")
        .to_string();
    let output_filename = format!("burned_{display_stem}.mp4");

    let job_start = Instant::now();

    // watch coalesces: the editor task always renders the newest text, in order, one at a time.
    let (tick_tx, mut tick_rx) = tokio::sync::watch::channel(String::new());
    let api_tick = api.clone();
    crate::app::spawn_user_task(async move {
        while tick_rx.changed().await.is_ok() {
            let text = tick_rx.borrow_and_update().clone();
            if text.is_empty() {
                continue;
            }
            let _ = api_tick
                .edit_message_text(
                    &EditMessageTextParams::builder()
                        .chat_id(chat_id)
                        .message_id(status_msg_id)
                        .text(&text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(job_cancel_keyboard())
                        .build(),
                )
                .await;
        }
    });

    let cores_cl = cores.clone();
    let cancel_cl = cancel_flag.clone();
    let total_duration = meta.duration_secs.max(1);
    let ffmpeg_bin = crate::config::ffmpeg_path();
    let input_path = v_info.local_path.clone();
    let out_path_cl = output_path.clone();
    let log_path_cl = log_path.clone();
    let source_codec = meta.codec.clone();

    let burn_res = tokio::task::spawn_blocking(move || -> Result<(), String> {
        pin_current_thread(&cores_cl, trace_id);
        run_ffmpeg_burn(
            &ffmpeg_bin,
            &input_path,
            &filter_arg,
            &threads_arg,
            &source_codec,
            &out_path_cl,
            &log_path_cl,
            total_duration,
            job_start,
            &cancel_cl,
            tick_tx,
        )
    })
    .await;

    release_cpu(cores, trace_id).await;
    trim_memory();

    if cancel_flag.load(Ordering::Relaxed) {
        log_ev!("studio_burn", trace_id, "cancelled", "stage" => "burn");
        record_event_user(user_id, "studio_burn", "burn", "cancelled", 1).await;
        fail!("studio.burn.job_cancelled");
        return;
    }

    let burn_err = match burn_res {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(e),
        Err(e) => Some(format!("join error: {e}")),
    };
    if let Some(e) = burn_err {
        let tail = read_log_tail(&log_path);
        log_ev!("studio_burn", trace_id, "burn_failed", "=>" => format!("fail err={e} ffmpeg={tail}"));
        record_error_global("studio_burn", format!("burn failed: {e}; ffmpeg: {tail}")).await;
        record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
        fail!("studio.burn.error.burn_failed");
        return;
    }

    let output_bytes = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(0);
    if output_bytes == 0 {
        let tail = read_log_tail(&log_path);
        log_ev!("studio_burn", trace_id, "output_empty", "=>" => format!("fail ffmpeg={tail}"));
        record_error_global("studio_burn", format!("empty output; ffmpeg: {tail}")).await;
        record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
        fail!("studio.burn.error.burn_failed");
        return;
    }
    let burn_duration_secs = job_start.elapsed().as_secs();
    log_ev!("studio_burn", trace_id, "burn_done", "secs" => burn_duration_secs, "bytes" => output_bytes);

    // Over the cap the output is cut into pieces instead of thrown away: a hardsub re-encode can
    // still cross 2000 MB even with the encoder matched to the source.
    let mut parts: Vec<PathBuf> = vec![output_path.clone()];
    if output_bytes > MAX_UPLOAD_BYTES {
        let part_count = upload_part_count(output_bytes, MAX_UPLOAD_BYTES);
        log_ev!("studio_burn", trace_id, "output_oversized", "bytes" => output_bytes, "parts" => part_count);

        let split_text = apply_premium_to_md(&tf(
            "studio.burn.status_splitting",
            &[("parts", &md_escape(&part_count.to_string()))],
        ));
        let _ = api
            .edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .text(&split_text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(job_cancel_keyboard())
                    .build(),
            )
            .await;

        let ffmpeg_bin_split = crate::config::ffmpeg_path();
        let in_path = output_path.clone();
        let dir_cl = work_dir.to_path_buf();
        let split_res = tokio::task::spawn_blocking(move || {
            split_video_into_parts(
                &ffmpeg_bin_split,
                &in_path,
                &dir_cl,
                total_duration,
                part_count,
            )
        })
        .await;

        let split_parts = match split_res {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                log_ev!("studio_burn", trace_id, "split_failed", "=>" => format!("fail err={e}"));
                record_error_global("studio_burn", format!("split failed: {e}")).await;
                record_event_user(user_id, "studio_burn", "burn", "oversized", 1).await;
                fail!("studio.burn.error.oversized");
                return;
            }
            Err(e) => {
                log_ev!("studio_burn", trace_id, "split_failed", "=>" => format!("fail join={e}"));
                record_error_global("studio_burn", format!("split join failed: {e}")).await;
                record_event_user(user_id, "studio_burn", "burn", "oversized", 1).await;
                fail!("studio.burn.error.oversized");
                return;
            }
        };

        // Keyframe-aligned cuts are approximate; a piece still over the cap cannot be sent.
        if let Some(big) = split_parts
            .iter()
            .find(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) > MAX_UPLOAD_BYTES)
        {
            log_ev!("studio_burn", trace_id, "split_part_oversized", "part" => big.display().to_string());
            record_event_user(user_id, "studio_burn", "burn", "oversized", 1).await;
            fail!("studio.burn.error.oversized");
            return;
        }

        record_event_user(user_id, "studio_burn", "burn", "split", 1).await;
        log_ev!("studio_burn", trace_id, "split_done", "parts" => split_parts.len());
        parts = split_parts;
    }

    if cancel_flag.load(Ordering::Relaxed) {
        log_ev!("studio_burn", trace_id, "cancelled", "stage" => "pre_upload");
        record_event_user(user_id, "studio_burn", "burn", "cancelled", 1).await;
        fail!("studio.burn.job_cancelled");
        return;
    }

    let total_parts = parts.len();
    let mut upload_err: Option<String> = None;

    for (idx, part_path) in parts.iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            log_ev!("studio_burn", trace_id, "cancelled", "stage" => "upload");
            record_event_user(user_id, "studio_burn", "burn", "cancelled", 1).await;
            fail!("studio.burn.job_cancelled");
            return;
        }

        let upload_text = apply_premium_to_md(&t("studio.burn.status_uploading"));
        let _ = api
            .edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .text(&upload_text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(job_cancel_keyboard())
                    .build(),
            )
            .await;

        let thumb_path = work_dir.join(format!("thumb_{idx}.jpg"));
        extract_thumbnail(part_path, &thumb_path);

        let caption = if total_parts > 1 {
            apply_premium_to_md(&tf(
                "studio.burn.job_done_part",
                &[
                    ("filename", &md_escape(&output_filename)),
                    ("burn_time", &md_escape(&format_eta_hms(burn_duration_secs))),
                    ("part", &md_escape(&(idx + 1).to_string())),
                    ("total", &md_escape(&total_parts.to_string())),
                ],
            ))
        } else {
            apply_premium_to_md(&tf(
                "studio.burn.job_done",
                &[
                    ("filename", &md_escape(&output_filename)),
                    ("burn_time", &md_escape(&format_eta_hms(burn_duration_secs))),
                ],
            ))
        };

        let mut params = SendVideoParams::builder()
            .chat_id(chat_id)
            .video(FileUpload::InputFile(InputFile {
                path: part_path.clone(),
            }))
            .caption(&caption)
            .parse_mode(ParseMode::MarkdownV2)
            .supports_streaming(true)
            .build();

        if let Ok(out_meta) = super::trim::run_ffprobe(part_path) {
            if out_meta.width > 0 {
                params.width = Some(out_meta.width);
            }
            if out_meta.height > 0 {
                params.height = Some(out_meta.height);
            }
            if out_meta.duration_secs > 0 {
                params.duration = Some(out_meta.duration_secs as u32);
            }
        }
        if std::fs::metadata(&thumb_path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            params.thumbnail = Some(FileUpload::InputFile(InputFile {
                path: thumb_path.clone(),
            }));
        }

        let send_res = send_file_with_upload_ticker::<_, Message>(
            api,
            "sendVideo",
            &params,
            part_path,
            chat_id,
            status_msg_id,
            "transfer.stage.sending_video",
            Some(cancel_flag.clone()),
        )
        .await;

        if let Err(e) = send_res {
            upload_err = Some(format!("part {}/{total_parts}: {e}", idx + 1));
            break;
        }
    }

    let _ = api
        .delete_message(
            &DeleteMessageParams::builder()
                .chat_id(chat_id)
                .message_id(status_msg_id)
                .build(),
        )
        .await;

    match upload_err {
        None => {
            log_ev!("studio_burn", trace_id, "success", "burn_duration" => burn_duration_secs, "parts" => total_parts);
            record_event_user(user_id, "studio_burn", "burn_success", "ok", 1).await;
            record_event_global("studio_burn", "burn_success", "", 1).await;
        }
        Some(e) => {
            log_ev!("studio_burn", trace_id, "upload_failed", "=>" => format!("fail err={e}"));
            record_error_global("studio_burn", format!("upload failed: {e}")).await;
            record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
            let _ = send_text_md(api, chat_id, &t("studio.burn.error.burn_failed")).await;
        }
    }

    remove_active_job(user_id);
    rearm_burn_prompt(api, chat_id, user_id, &flow_manager).await;
}

/// Runs ffmpeg to completion. The stdout `-progress` reader lives on its own thread so the
/// cancel check never waits on a line that a stalled ffmpeg will not print.
#[allow(clippy::too_many_arguments)]
fn run_ffmpeg_burn(
    ffmpeg_bin: &Path,
    input: &Path,
    filter_arg: &str,
    threads_arg: &str,
    source_codec: &str,
    output: &Path,
    log_path: &Path,
    total_duration: u64,
    job_start: Instant,
    cancel: &Arc<AtomicBool>,
    tick_tx: tokio::sync::watch::Sender<String>,
) -> Result<(), String> {
    let log_file =
        std::fs::File::create(log_path).map_err(|e| format!("ffmpeg log create failed: {e}"))?;

    let mut child = std::process::Command::new(ffmpeg_bin)
        .args(["-y", "-hide_banner", "-nostdin", "-i"])
        .arg(input)
        .args(["-map", "0:v:0", "-map", "0:a:0?", "-vf", filter_arg])
        .args(video_encoder_args(source_codec))
        .args([
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-movflags",
            "+faststart",
            "-threads",
            threads_arg,
            "-progress",
            "pipe:1",
        ])
        .arg(output)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::from(log_file))
        .spawn()
        .map_err(|e| format!("ffmpeg spawn failed: {e}"))?;

    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let Some(out) = stdout else { return };
        let mut current_us = 0u64;
        let mut speed_str = "1.0x".to_string();
        let mut last_pct = u64::MAX;
        let mut last_edit = Instant::now() - TICKER_MIN_INTERVAL;

        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(val) = line.strip_prefix("out_time_us=") {
                current_us = val.trim().parse::<u64>().unwrap_or(0);
            } else if let Some(val) = line.strip_prefix("speed=") {
                speed_str = val.trim().to_string();
            } else if line.starts_with("progress=") {
                let current_secs = current_us / 1_000_000;
                let pct = ((current_secs * 100) / total_duration).min(100);
                if pct == last_pct && last_edit.elapsed() < TICKER_MIN_INTERVAL {
                    continue;
                }
                last_pct = pct;
                last_edit = Instant::now();

                let speed_num = speed_str
                    .trim_end_matches('x')
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let eta_secs = if speed_num > 0.0 && total_duration > current_secs {
                    ((total_duration - current_secs) as f64 / speed_num) as u64
                } else {
                    0
                };

                let text = apply_premium_to_md(&tf(
                    "studio.burn.status_burning",
                    &[
                        (
                            "elapsed",
                            &md_escape(&format_eta_hms(job_start.elapsed().as_secs())),
                        ),
                        ("pct", &pct.to_string()),
                        ("speed", &md_escape(&speed_str)),
                        ("eta", &md_escape(&format_eta_hms(eta_secs))),
                    ],
                ));
                let _ = tick_tx.send(text);
            }
        }
    });

    let mut cancelled = false;
    let mut wait_err: Option<String> = None;
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            let _ = child.kill();
            break None;
        }
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => std::thread::sleep(Duration::from_millis(300)),
            Err(e) => {
                wait_err = Some(format!("try_wait failed: {e}"));
                let _ = child.kill();
                break None;
            }
        }
    };

    // Always reap, or a cancelled burn leaves a zombie behind.
    let _ = child.wait();
    let _ = reader.join();

    if cancelled {
        return Err("cancelled".to_string());
    }
    if let Some(e) = wait_err {
        return Err(e);
    }
    match status {
        Some(s) if s.success() => Ok(()),
        Some(s) => Err(format!("ffmpeg exited with {:?}", s.code())),
        None => Err("ffmpeg produced no exit status".to_string()),
    }
}

fn extract_thumbnail(video: &Path, thumb: &Path) {
    let _ = std::process::Command::new(crate::config::ffmpeg_path())
        .args([
            "-y",
            "-hide_banner",
            "-nostdin",
            "-ss",
            "00:00:00.500",
            "-i",
        ])
        .arg(video)
        .args(["-vframes", "1", "-q:v", "3"])
        .arg(thumb)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Last 400 chars of ffmpeg's stderr log — enough to name the real failure in one log line.
fn read_log_tail(log_path: &Path) -> String {
    let raw = std::fs::read_to_string(log_path).unwrap_or_default();
    let cleaned = raw.replace('\n', " ").trim().to_string();
    let chars: Vec<char> = cleaned.chars().collect();
    if chars.len() <= 400 {
        cleaned
    } else {
        chars[chars.len() - 400..].iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_subtitle_format() {
        assert_eq!(
            detect_subtitle_format("movie.srt"),
            Some(SubtitleFormat::Srt)
        );
        assert_eq!(
            detect_subtitle_format("movie.ASS"),
            Some(SubtitleFormat::Ass)
        );
        assert_eq!(
            detect_subtitle_format("movie.ssa"),
            Some(SubtitleFormat::Ass)
        );
        assert_eq!(
            detect_subtitle_format("movie.vtt"),
            Some(SubtitleFormat::Vtt)
        );
        assert_eq!(detect_subtitle_format("movie.txt"), None);
    }

    #[test]
    fn test_escape_ffmpeg_filter_path_uses_filtergraph_rules() {
        // Not shell rules: ffmpeg parses the filtergraph itself, so a single backslash is correct.
        let p = Path::new("/tmp/dir:with_colon/sub'file.ass");
        assert_eq!(
            escape_ffmpeg_filter_path(p),
            "/tmp/dir\\:with_colon/sub\\'file.ass"
        );
        assert_eq!(escape_filter_value("a,b=c[d];e"), "a\\,b\\=c\\[d\\]\\;e");
        assert_eq!(escape_filter_value("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn test_upload_part_count_covers_oversized_output() {
        const CAP: u64 = MAX_UPLOAD_BYTES;
        // 2402 MB output: halved, both pieces land under the cap.
        assert_eq!(upload_part_count(2402 * 1024 * 1024, CAP), 2);
        // Just over the cap still halves rather than failing.
        assert_eq!(upload_part_count(CAP + 1, CAP), 2);
        // Far over the cap needs more than two pieces, or a "half" would still be unsendable.
        assert_eq!(upload_part_count(5000 * 1024 * 1024, CAP), 3);
        assert_eq!(upload_part_count(20_000 * 1024 * 1024, CAP), 10);
        // Every piece must fit: bytes/parts is never above the cap.
        for mb in [2001u64, 2402, 3999, 4001, 9000, 40_000] {
            let bytes = mb * 1024 * 1024;
            let parts = upload_part_count(bytes, CAP);
            assert!(parts >= 2, "{mb} MB must be split");
            assert!(
                bytes.div_ceil(parts) <= CAP,
                "{mb} MB in {parts} parts still exceeds the cap"
            );
        }
        // Never divides by zero.
        assert_eq!(upload_part_count(1, 0), 2);
    }

    #[test]
    fn test_video_encoder_args_matches_source_codec() {
        // An AV1 source must not come back as x264 — that is what blew the 2000 MB upload cap.
        assert_eq!(video_encoder_args("av1")[1], "libsvtav1");
        assert_eq!(video_encoder_args("AV1")[1], "libsvtav1");
        assert_eq!(video_encoder_args("hevc")[1], "libx265");
        assert_eq!(video_encoder_args("h265")[1], "libx265");
        assert_eq!(video_encoder_args("vp9")[1], "libvpx-vp9");
        // Fallback keeps the widest-compatibility encoder.
        assert_eq!(video_encoder_args("h264")[1], "libx264");
        assert_eq!(video_encoder_args("unknown")[1], "libx264");
        assert_eq!(video_encoder_args("")[1], "libx264");
        // pix_fmt is only forced on the x264 path.
        assert!(video_encoder_args("h264").contains(&"yuv420p"));
        assert!(!video_encoder_args("av1").contains(&"yuv420p"));
    }

    #[test]
    fn test_build_filter_arg_escapes_force_style() {
        let ass = build_filter_arg(SubtitleFormat::Ass, Path::new("/tmp/w/sub.ass"));
        assert_eq!(ass, "ass=filename=/tmp/w/sub.ass");

        let srt = build_filter_arg(SubtitleFormat::Srt, Path::new("/tmp/w/sub.srt"));
        assert!(srt.starts_with("subtitles=filename=/tmp/w/sub.srt:force_style="));
        // Style separators must be escaped or ffmpeg reads them as extra filter options.
        let style = srt.split("force_style=").nth(1).unwrap();
        assert!(!style.contains("\\\\,"), "no double-escaping");
        assert!(srt.contains("\\,Fontsize"));
        assert!(srt.contains("Fontname\\=Arial"));
    }

    #[test]
    fn test_subtitle_ext_is_fixed_and_safe() {
        // Work-dir names never derive from user input, so traversal cannot happen.
        assert_eq!(SubtitleFormat::Srt.ext(), "srt");
        assert_eq!(SubtitleFormat::Ass.ext(), "ass");
        assert_eq!(SubtitleFormat::Vtt.ext(), "vtt");
        assert_eq!(sanitize_filename("../../etc/passwd"), "....etcpasswd");
    }
}
