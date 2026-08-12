//! Video trim and edit module for Photo & Video Magic Studio (`studio_trim`).
//!
//! Handles ffprobe metadata extraction, multi-range timestamp parsing (with Persian/Arabic-Indic
//! digit normalization and whitespace tolerance), brokered ffmpeg trimming with copy -> encode fallback,
//! live progress ticker, cancellation, and re-arming back to range collection state.

use std::path::Path;
use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use regex::Regex;
use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{DeleteMessageParams, EditMessageTextParams, SendVideoParams},
    types::{InlineKeyboardMarkup, Message, ReplyParameters},
};

use crate::bot::{
    files::download_telegram_file,
    messaging::{send_text_md, send_text_md_with_keyboard},
};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::panel::btn_icon_danger;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::log::next_trace_id;
use crate::moebius::cpu::{acquire_cpu, pin_current_thread, release_cpu, trim_memory};
use crate::rank::quota::add_traffic;
use crate::stats::record_event_user;

use super::pipeline::{TempDirGuard, register_active_job, remove_active_job};

pub const DEFAULT_MAX_CUT_RANGES: usize = 10;

static TIMESTAMP_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<start>(?:\d{1,2}:)?\d{1,2}:\d{2})\s*(?:[-–—~]|->|=>|\bto\b|\bتا\b)\s*(?P<end>(?:\d{1,2}:)?\d{1,2}:\d{2})")
        .expect("Valid timestamp range regex")
});

/// Normalizes Persian (`۰-۹`) and Arabic-Indic (`٠-٩`) digits to ASCII (`0-9`).
pub fn normalize_digits(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '۰'..='۹' => (c as u32 - '۰' as u32 + '0' as u32) as u8 as char,
            '٠'..='٩' => (c as u32 - '٠' as u32 + '0' as u32) as u8 as char,
            other => other,
        })
        .collect()
}

/// Converts timestamp string (`HH:MM:SS` or `MM:SS`) to total seconds.
pub fn parse_timestamp(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    match parts.len() {
        2 => {
            let mins: u64 = parts[0].parse().ok()?;
            let secs: u64 = parts[1].parse().ok()?;
            if secs >= 60 {
                return None;
            }
            Some(mins * 60 + secs)
        }
        3 => {
            let hours: u64 = parts[0].parse().ok()?;
            let mins: u64 = parts[1].parse().ok()?;
            let secs: u64 = parts[2].parse().ok()?;
            if mins >= 60 || secs >= 60 {
                return None;
            }
            Some(hours * 3600 + mins * 60 + secs)
        }
        _ => None,
    }
}

/// Formats seconds into `HH:MM:SS` string.
pub fn format_timestamp(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{hours:02}:{mins:02}:{s:02}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutRange {
    pub start_secs: u64,
    pub end_secs: u64,
    pub raw_line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RangeError {
    #[error("No valid cut ranges found in input")]
    NoValidRanges,
    #[error("Line {line_idx}: Invalid format '{text}' (expected MM:SS - MM:SS or HH:MM:SS - HH:MM:SS)")]
    InvalidFormat { line_idx: usize, text: String },
    #[error("Line {line_idx}: Start time ({start}s) is greater than or equal to end time ({end}s)")]
    StartGteEnd {
        line_idx: usize,
        start: u64,
        end: u64,
    },
    #[error(
        "Line {line_idx}: End time ({end}s) exceeds video duration ({duration}s)"
    )]
    EndExceedsDuration {
        line_idx: usize,
        end: u64,
        duration: u64,
    },
    #[error("Too many cut ranges specified (max {max})")]
    ExceedsMaxRanges { max: usize },
}

/// Parses and validates cut ranges extracted from user input (including embedded in long text).
pub fn parse_cut_ranges(
    input: &str,
    duration_secs: u64,
    max_ranges: usize,
) -> Result<Vec<CutRange>, Vec<RangeError>> {
    let normalized = normalize_digits(input);
    let mut ranges = Vec::new();
    let mut errors = Vec::new();

    let matches: Vec<_> = TIMESTAMP_RANGE_RE.captures_iter(&normalized).collect();

    if matches.is_empty() {
        return Err(vec![RangeError::NoValidRanges]);
    }

    if matches.len() > max_ranges {
        return Err(vec![RangeError::ExceedsMaxRanges { max: max_ranges }]);
    }

    for (match_idx, cap) in matches.iter().enumerate() {
        let line_idx = match_idx + 1;
        let start_str = &cap["start"];
        let end_str = &cap["end"];

        let start_opt = parse_timestamp(start_str);
        let end_opt = parse_timestamp(end_str);

        match (start_opt, end_opt) {
            (Some(start), Some(end)) => {
                let end_clamped = end.min(duration_secs);
                if start >= end_clamped {
                    if start >= end {
                        errors.push(RangeError::StartGteEnd {
                            line_idx,
                            start,
                            end,
                        });
                    } else {
                        errors.push(RangeError::EndExceedsDuration {
                            line_idx,
                            end,
                            duration: duration_secs,
                        });
                    }
                } else {
                    ranges.push(CutRange {
                        start_secs: start,
                        end_secs: end_clamped,
                        raw_line: cap[0].to_string(),
                    });
                }
            }
            _ => {
                errors.push(RangeError::InvalidFormat {
                    line_idx,
                    text: cap[0].to_string(),
                });
            }
        }
    }

    if !errors.is_empty() {
        Err(errors)
    } else {
        Ok(ranges)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VideoMetadata {
    pub filename: String,
    pub width: u32,
    pub height: u32,
    pub bitrate: u64,
    pub fps: u32,
    pub codec: String,
    pub duration_secs: u64,
}

/// Runs `ffprobe` to extract video metadata.
pub fn run_ffprobe(video_path: &Path) -> anyhow::Result<VideoMetadata> {
    let output = std::process::Command::new(crate::config::ffprobe_path())
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration,bit_rate:stream=width,height,r_frame_rate,codec_name",
            "-of",
            "json",
        ])
        .arg(video_path)
        .output()
        .map_err(|e| anyhow::anyhow!("failed to execute ffprobe: {e}"))?;

    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffprobe failed: {err_msg}");
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let format = json.get("format");
    let streams = json.get("streams").and_then(|s| s.as_array());

    let duration_secs = format
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|d| d.round() as u64)
        .unwrap_or(0);

    let bitrate = format
        .and_then(|f| f.get("bit_rate"))
        .and_then(|b| b.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let video_stream = streams.and_then(|arr| {
        arr.iter().find(|st| {
            st.get("width").is_some() || st.get("codec_name").is_some()
        })
    });

    let width = video_stream
        .and_then(|s| s.get("width"))
        .and_then(|w| w.as_u64())
        .unwrap_or(0) as u32;

    let height = video_stream
        .and_then(|s| s.get("height"))
        .and_then(|h| h.as_u64())
        .unwrap_or(0) as u32;

    let codec = video_stream
        .and_then(|s| s.get("codec_name"))
        .and_then(|c| c.as_str())
        .unwrap_or("unknown")
        .to_string();

    let fps = video_stream
        .and_then(|s| s.get("r_frame_rate"))
        .and_then(|r| r.as_str())
        .map(|rate_str| {
            let parts: Vec<&str> = rate_str.split('/').collect();
            if parts.len() == 2 {
                let num: f64 = parts[0].parse().unwrap_or(0.0);
                let den: f64 = parts[1].parse().unwrap_or(1.0);
                if den > 0.0 {
                    (num / den).round() as u32
                } else {
                    0
                }
            } else {
                rate_str.parse::<u32>().unwrap_or(0)
            }
        })
        .unwrap_or(0);

    let filename = video_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "video.mp4".to_string());

    Ok(VideoMetadata {
        filename,
        width,
        height,
        bitrate,
        fps,
        codec,
        duration_secs,
    })
}

pub fn cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("studio.trim.cancel_btn"),
            crate::bot::constants::CB_STUDIO_TRIM_CANCEL,
            "cancel",
        )]])
        .build()
}

pub fn job_cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("studio.trim.cancel_btn"),
            crate::bot::constants::CB_STUDIO_TRIM_JOBCANCEL,
            "cancel",
        )]])
        .build()
}

/// Handles video upload when flow state is `AwaitingStudioTrimVideo`.
pub async fn handle_video_upload(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    let msg_id = message.message_id;

    log_actor_id!("studio_trim", trace_id, user_id, "uploaded" => "video");
    log_ev!("studio_trim", trace_id, "video_received", "user_id" => user_id, "msg_id" => msg_id);

    if !super::is_video_message_metadata(message) {
        log_ev!("studio_trim", trace_id, "not_a_video_metadata", "=>" => "fail");
        let _ = send_text_md(api, chat_id, &t("studio.trim.error.not_a_video")).await;
        return;
    }

    let file_id = message
        .video
        .as_ref()
        .map(|v| v.file_id.clone())
        .or_else(|| {
            message
                .document
                .as_ref()
                .map(|d| d.file_id.clone())
        });

    let Some(file_id) = file_id else {
        log_ev!("studio_trim", trace_id, "invalid_video", "=>" => "fail");
        let _ = send_text_md(api, chat_id, &t("studio.trim.error.not_a_video")).await;
        return;
    };

    let orig_filename = message
        .video
        .as_ref()
        .and_then(|v| v.file_name.as_deref())
        .or_else(|| {
            message
                .document
                .as_ref()
                .and_then(|d| d.file_name.as_deref())
        })
        .unwrap_or("video.mp4")
        .to_string();

    // 1. Reply with initial status: "در حال دانلود ویدئو..."
    let status_raw = tf(
        "studio.trim.status_downloading",
        &[("elapsed", &md_escape("0s")), ("detail", "")],
    );
    let status_text = apply_premium_to_md(&status_raw);
    let params = frankenstein::methods::SendMessageParams::builder()
        .chat_id(chat_id)
        .reply_parameters(ReplyParameters::builder().message_id(msg_id).build())
        .text(&status_text)
        .parse_mode(ParseMode::MarkdownV2)
        .build();

    let status_msg_id = match api.send_message(&params).await {
        Ok(resp) => resp.result.message_id,
        Err(e) => {
            log_ev!("studio_trim", trace_id, "status_send_failed", "=>" => format!("fail err={e}"));
            return;
        }
    };

    // Download file
    let work_dir = std::env::temp_dir().join(format!("studio_trim_{trace_id}_{user_id}"));
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        log_ev!("studio_trim", trace_id, "mkdir_failed", "=>" => format!("fail err={e}"));
        let _ = api.delete_message(&DeleteMessageParams::builder().chat_id(chat_id).message_id(status_msg_id).build()).await;
        let _ = send_text_md(api, chat_id, &t("studio.trim.error.download_failed")).await;
        return;
    }
    let _guard = TempDirGuard::new(work_dir.clone());
    let total_bytes = message
        .video
        .as_ref()
        .and_then(|v| v.file_size)
        .or_else(|| message.document.as_ref().and_then(|d| d.file_size))
        .unwrap_or(0);

    let local_file = work_dir.join(&orig_filename);

    let dl_stop_flag = super::pipeline::spawn_download_ticker(
        api.clone(),
        chat_id,
        status_msg_id,
        local_file.clone(),
        total_bytes,
        "studio.trim",
        None,
    );

    let dl_res = download_telegram_file(api, &file_id, &local_file).await;
    dl_stop_flag.store(true, Ordering::Relaxed);

    if let Err(e) = dl_res {
        log_ev!("studio_trim", trace_id, "download_failed", "=>" => format!("fail err={e}"));
        let _ = api.delete_message(&DeleteMessageParams::builder().chat_id(chat_id).message_id(status_msg_id).build()).await;
        let _ = send_text_md(api, chat_id, &t("studio.trim.error.download_failed")).await;
        return;
    }


    // Edit status to "در حال پردازش..."
    let processing_text = apply_premium_to_md(&t("studio.trim.status_processing"));
    let _ = api.edit_message_text(
        &EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(status_msg_id)
            .text(&processing_text)
            .parse_mode(ParseMode::MarkdownV2)
            .build()
    ).await;

    // Run ffprobe
    let meta = match run_ffprobe(&local_file) {
        Ok(m) => m,
        Err(e) => {
            log_ev!("studio_trim", trace_id, "ffprobe_failed", "=>" => format!("fail err={e}"));
            let _ = api.delete_message(&DeleteMessageParams::builder().chat_id(chat_id).message_id(status_msg_id).build()).await;
            let _ = send_text_md(api, chat_id, &t("studio.trim.error.ffprobe_failed")).await;
            return;
        }
    };

pub fn format_bitrate(bps: u64) -> String {
    if bps == 0 {
        "N/A".to_string()
    } else {
        let kbps = bps / 1000;
        format!("{kbps} kbps")
    }
}

    // Edit status message into metadata prompt with start button
    let duration_formatted = format_timestamp(meta.duration_secs);
    let bitrate_formatted = format_bitrate(meta.bitrate);
    let raw_info = tf(
        "studio.trim.info_template",
        &[
            ("filename", &md_escape(&meta.filename)),
            ("width", &meta.width.to_string()),
            ("height", &meta.height.to_string()),
            ("bitrate", &md_escape(&bitrate_formatted)),
            ("fps", &meta.fps.to_string()),
            ("codec", &md_escape(&meta.codec)),
            ("duration", &md_escape(&duration_formatted)),
        ],
    );
    let info_text = apply_premium_to_md(&raw_info);

    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![crate::emoji::panel::btn_icon(
                &t("studio.trim.start_trim_btn"),
                crate::bot::constants::CB_STUDIO_TRIM_START,
                "edit",
            )],
            vec![btn_icon_danger(
                &t("studio.trim.cancel_btn"),
                crate::bot::constants::CB_STUDIO_TRIM_CANCEL,
                "cancel",
            )],
        ])
        .build();

    let edit_params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(status_msg_id)
        .text(&info_text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(kb)
        .build();

    match api.edit_message_text(&edit_params).await {
        Ok(_) => {
            log_ev!("studio_trim", trace_id, "meta_sent", "duration" => meta.duration_secs);
            flow_manager.set(
                user_id,
                FlowState::AwaitingStudioTrimRanges {
                    file_id,
                    filename: orig_filename,
                    duration_secs: meta.duration_secs,
                },
            );
        }
        Err(e) => {
            log_ev!("studio_trim", trace_id, "send_meta_failed", "=>" => format!("fail err={e}"));
            let _ = send_text_md(api, chat_id, &t("studio.trim.error.ffprobe_failed")).await;
        }
    }
}

/// Handles multi-line cut ranges text input when flow state is `AwaitingStudioTrimRanges`.
pub async fn handle_ranges_input(
    api: &Bot,
    message: &Message,
    user_id: i64,
    file_id: &str,
    filename: &str,
    duration_secs: u64,
    flow_manager: &FlowManager,
    database: Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    let msg_text = message.text.as_deref().unwrap_or_default();

    log_actor_id!("studio_trim", trace_id, user_id, "submitted" => "ranges");
    log_ev!("studio_trim", trace_id, "ranges_received", "len" => msg_text.len());

    let parsed = parse_cut_ranges(msg_text, duration_secs, DEFAULT_MAX_CUT_RANGES);
    let ranges = match parsed {
        Ok(r) => r,
        Err(errors) => {
            log_ev!("studio_trim", trace_id, "ranges_validation_failed", "err_count" => errors.len());
            let mut err_msgs = Vec::new();
            for err in errors {
                match err {
                    RangeError::InvalidFormat { line_idx, text } => {
                        err_msgs.push(tf(
                            "studio.trim.error.invalid_format",
                            &[
                                ("line", &line_idx.to_string()),
                                ("text", &md_escape(&text)),
                            ],
                        ));
                    }
                    RangeError::StartGteEnd { line_idx, .. } => {
                        err_msgs.push(tf(
                            "studio.trim.error.start_gte_end",
                            &[("line", &line_idx.to_string())],
                        ));
                    }
                    RangeError::EndExceedsDuration {
                        line_idx,
                        end,
                        duration,
                    } => {
                        err_msgs.push(tf(
                            "studio.trim.error.end_exceeds_duration",
                            &[
                                ("line", &line_idx.to_string()),
                                ("end", &md_escape(&format_timestamp(end))),
                                ("duration", &md_escape(&format_timestamp(duration))),
                            ],
                        ));
                    }
                    RangeError::ExceedsMaxRanges { max } => {
                        err_msgs.push(tf(
                            "studio.trim.error.max_ranges",
                            &[("max", &max.to_string())],
                        ));
                    }
                    RangeError::NoValidRanges => {
                        err_msgs.push(t("studio.trim.error.no_valid_ranges"));
                    }
                }
            }
            let full_err = apply_premium_to_md(&err_msgs.join("\n"));
            let _ = send_text_md(api, chat_id, &full_err).await;
            return;
        }
    };

    // Spawn long job for video trimming
    let api_clone = api.clone();
    let file_id_clone = file_id.to_string();
    let filename_clone = filename.to_string();
    let flow_manager_clone = flow_manager.clone();

    crate::app::spawn_user_task(async move {
        execute_trim_job(
            &api_clone,
            chat_id,
            user_id,
            &file_id_clone,
            &filename_clone,
            duration_secs,
            ranges,
            &flow_manager_clone,
            database,
        )
        .await;
    });
}

/// Executes brokered ffmpeg multi-cut job with ticker, cancel flag, and re-arm.
pub async fn execute_trim_job(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    file_id: &str,
    filename: &str,
    duration_secs: u64,
    ranges: Vec<CutRange>,
    flow_manager: &FlowManager,
    database: Option<PostgresDatabase>,
) {
    if crate::moebius::cpu::is_user_cpu_busy(user_id).await {
        let _ = crate::bot::send_text_md(api, chat_id, &t("active_job_running")).await;
        return;
    }

    let trace_id = next_trace_id();
    log_ev!("studio_trim", trace_id, "execute_start", "ranges_count" => ranges.len());

    let wall_start = Instant::now(); // measures total wall time including download

    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_active_job(user_id, cancel_flag.clone());

    // 1. Initial ticker message
    let status_raw = tf(
        "studio.trim.status_downloading",
        &[("elapsed", &md_escape("0s")), ("detail", "")],
    );
    let initial_status = apply_premium_to_md(&status_raw);
    let params = frankenstein::methods::SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&initial_status)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(job_cancel_keyboard()))
        .build();

    let status_msg_id = match api.send_message(&params).await {
        Ok(resp) => resp.result.message_id,
        Err(e) => {
            log_ev!("studio_trim", trace_id, "job_status_send_failed", "=>" => format!("fail err={e}"));
            remove_active_job(user_id);
            return;
        }
    };

    let work_dir = std::env::temp_dir().join(format!("studio_trim_job_{trace_id}_{user_id}"));
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        log_ev!("studio_trim", trace_id, "mkdir_failed", "=>" => format!("fail err={e}"));
        remove_active_job(user_id);
        let _ = api.delete_message(&DeleteMessageParams::builder().chat_id(chat_id).message_id(status_msg_id).build()).await;
        let _ = send_text_md(api, chat_id, &t("studio.trim.error.download_failed")).await;
        return;
    }
    let _guard = TempDirGuard::new(work_dir.clone());
    let source_video = work_dir.join(filename);

    if cancel_flag.load(Ordering::Relaxed) {
        remove_active_job(user_id);
        let _ = api.delete_message(&DeleteMessageParams::builder().chat_id(chat_id).message_id(status_msg_id).build()).await;
        let _ = send_text_md(api, chat_id, &t("studio.trim.job_cancelled")).await;
        return;
    }

    let stats_job_id = crate::stats::record_download_start(user_id, "studio_trim").await;

    // Ingest source video
    let dl_stop_flag = super::pipeline::spawn_download_ticker(
        api.clone(),
        chat_id,
        status_msg_id,
        source_video.clone(),
        0,
        "studio.trim",
        Some(cancel_flag.clone()),
    );

    let dl_result = match download_telegram_file(api, file_id, &source_video).await {
        Ok(res) => {
            dl_stop_flag.store(true, Ordering::Relaxed);
            res
        }
        Err(e) => {
            dl_stop_flag.store(true, Ordering::Relaxed);
            log_ev!("studio_trim", trace_id, "download_failed", "=>" => format!("fail err={e}"));
            remove_active_job(user_id);
            let _ = api.delete_message(&DeleteMessageParams::builder().chat_id(chat_id).message_id(status_msg_id).build()).await;
            let _ = send_text_md(api, chat_id, &t("studio.trim.error.download_failed")).await;
            return;
        }
    };

    if let Some(jid) = stats_job_id {
        crate::stats::record_download_done(
            jid,
            dl_result.bytes as i64,
            None,
            None,
            Some(dl_result.speed_bps() as i64),
        )
        .await;
    }


    if cancel_flag.load(Ordering::Relaxed) {
        remove_active_job(user_id);
        let _ = api.delete_message(&DeleteMessageParams::builder().chat_id(chat_id).message_id(status_msg_id).build()).await;
        let _ = send_text_md(api, chat_id, &t("studio.trim.job_cancelled")).await;
        return;
    }

    // Acquire CPU broker cores
    let cores = acquire_cpu(user_id, trace_id).await;
    let threads_arg = if !cores.is_empty() {
        cores.len().to_string()
    } else {
        "2".to_string()
    };

    if cancel_flag.load(Ordering::Relaxed) {
        release_cpu(cores, trace_id).await;
        remove_active_job(user_id);
        let _ = api.delete_message(&DeleteMessageParams::builder().chat_id(chat_id).message_id(status_msg_id).build()).await;
        let _ = send_text_md(api, chat_id, &t("studio.trim.job_cancelled")).await;
        return;
    }

    let bot_username = crate::config::bot_username().to_string();

    // download_secs = time from very beginning to after download+acquire_cpu
    let download_secs = wall_start.elapsed().as_secs();
    let job_start = Instant::now(); // ticker counts from here
    let total_ranges = ranges.len();
    let mut total_upload_secs = 0u64;

    // Single outer live ticker — runs across all cuts
    let stop_ticker = Arc::new(AtomicBool::new(false));
    let current_cut = Arc::new(std::sync::atomic::AtomicUsize::new(1));
    {
        let stop_ticker_inner = stop_ticker.clone();
        let current_cut_inner = current_cut.clone();
        let api_inner = api.clone();
        crate::app::spawn_user_task(async move {
            let mut last_rendered = String::new();
            while !stop_ticker_inner.load(Ordering::Relaxed) {
                let elapsed_secs = job_start.elapsed().as_secs();
                let cur = current_cut_inner.load(Ordering::Relaxed);
                let elapsed_str = format!("{elapsed_secs}s");
                let text_key = format!("{cur}:{total_ranges}:{elapsed_secs}");
                if text_key != last_rendered {
                    last_rendered = text_key;
                    // ETA: avg secs per cut so far * remaining cuts
                    let completed = cur.saturating_sub(1);
                    let eta_param = if completed > 0 && cur <= total_ranges {
                        let avg = elapsed_secs as f64 / completed as f64;
                        let remaining = total_ranges - completed;
                        let eta_secs = (avg * remaining as f64) as u64;
                        let eta_str = format!("{eta_secs}s");
                        tf(
                            "studio.trim.status_job_ticker_eta",
                            &[("eta", &md_escape(&eta_str))],
                        )
                    } else {
                        String::new()
                    };
                    let raw_ticker = tf(
                        "studio.trim.status_job_ticker",
                        &[
                            ("current", &cur.to_string()),
                            ("total", &total_ranges.to_string()),
                            ("elapsed", &md_escape(&elapsed_str)),
                            ("eta", &eta_param),
                        ],
                    );
                    let ticker_text = apply_premium_to_md(&raw_ticker);
                    if let Err(e) = api_inner
                        .edit_message_text(
                            &EditMessageTextParams::builder()
                                .chat_id(chat_id)
                                .message_id(status_msg_id)
                                .text(&ticker_text)
                                .parse_mode(ParseMode::MarkdownV2)
                                .reply_markup(job_cancel_keyboard())
                                .build(),
                        )
                        .await
                    {
                        log_ev!("studio_trim", trace_id, "ticker_edit_failed", "cur" => cur, "elapsed" => elapsed_secs, "=>" => format!("err={e}"));
                    }
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
    }

    let loop_start = Instant::now();

    for (idx, range) in ranges.into_iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            log_ev!("studio_trim", trace_id, "job_cancelled_loop", "idx" => idx);
            break;
        }
        current_cut.store(idx + 1, Ordering::Relaxed);

        let output_name = format!("cut_{}_{}.mp4", idx + 1, trace_id);
        let output_path = work_dir.join(&output_name);

        let source_path = source_video.clone();
        let start_ss = format_timestamp(range.start_secs);
        let end_to = format_timestamp(range.end_secs);

        let raw_line = range.raw_line.clone();
        let cancel_flag_inner = cancel_flag.clone();
        let cores_inner = cores.clone();
        let threads_arg_inner = threads_arg.clone();
        let out_path_inner = output_path.clone();
        let work_dir_inner = work_dir.clone();
        let current_idx = idx + 1;

        let run_res = tokio::task::spawn_blocking(move || {
            pin_current_thread(&cores_inner, trace_id);

            // Fast copy attempt
            let mut child = match std::process::Command::new(crate::config::ffmpeg_path())
                .args([
                    "-y",
                    "-ss",
                    &start_ss,
                    "-to",
                    &end_to,
                    "-i",
                    source_path.to_str().unwrap_or_default(),
                    "-c",
                    "copy",
                    "-avoid_negative_ts",
                    "make_zero",
                    "-movflags",
                    "+faststart",
                    "-threads",
                    &threads_arg_inner,
                    out_path_inner.to_str().unwrap_or_default(),
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => return Err(anyhow::anyhow!("ffmpeg copy spawn error: {e}")),
            };

            let mut success = false;
            while !cancel_flag_inner.load(Ordering::Relaxed) {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        success = status.success()
                            && out_path_inner.exists()
                            && std::fs::metadata(&out_path_inner).map(|m| m.len() > 0).unwrap_or(false);
                        break;
                    }
                    Ok(None) => {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    Err(e) => return Err(anyhow::anyhow!("ffmpeg try_wait error: {e}")),
                }
            }

            if cancel_flag_inner.load(Ordering::Relaxed) {
                let _ = child.kill();
                return Ok(false);
            }

            if success {
                log_ev!("studio_trim", trace_id, "copy_success", "range" => &raw_line);
            } else {
                // Fallback to re-encode if copy failed
                log_ev!("studio_trim", trace_id, "copy_failed_fallback_encode", "range" => &raw_line);
                let mut encode_child = match std::process::Command::new(crate::config::ffmpeg_path())
                    .args([
                        "-y",
                        "-ss",
                        &start_ss,
                        "-to",
                        &end_to,
                        "-i",
                        source_path.to_str().unwrap_or_default(),
                        "-c:v",
                        "libx264",
                        "-c:a",
                        "aac",
                        "-preset",
                        "fast",
                        "-avoid_negative_ts",
                        "make_zero",
                        "-movflags",
                        "+faststart",
                        "-threads",
                        &threads_arg_inner,
                        out_path_inner.to_str().unwrap_or_default(),
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(e) => return Err(anyhow::anyhow!("ffmpeg encode spawn error: {e}")),
                };

                while !cancel_flag_inner.load(Ordering::Relaxed) {
                    match encode_child.try_wait() {
                        Ok(Some(status)) => {
                            success = status.success()
                                && out_path_inner.exists()
                                && std::fs::metadata(&out_path_inner).map(|m| m.len() > 0).unwrap_or(false);
                            break;
                        }
                        Ok(None) => {
                            std::thread::sleep(Duration::from_millis(200));
                        }
                        Err(e) => return Err(anyhow::anyhow!("ffmpeg try_wait error: {e}")),
                    }
                }

                if cancel_flag_inner.load(Ordering::Relaxed) {
                    let _ = encode_child.kill();
                    return Ok(false);
                }
            }

            if success {
                let thumb_name = format!("thumb_{}_{}.jpg", current_idx, trace_id);
                let thumb_path = work_dir_inner.join(&thumb_name);
                let _ = std::process::Command::new(crate::config::ffmpeg_path())
                    .args([
                        "-y",
                        "-ss",
                        "00:00:00.500",
                        "-i",
                        out_path_inner.to_str().unwrap_or_default(),
                        "-vframes",
                        "1",
                        "-q:v",
                        "3",
                        thumb_path.to_str().unwrap_or_default(),
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }

            Ok(success)
        })
        .await;

        let trim_ok = match run_res {
            Ok(Ok(true)) => true,
            _ => false,
        };

        if cancel_flag.load(Ordering::Relaxed) {
            break;
        }

        if !trim_ok {
            log_ev!("studio_trim", trace_id, "trim_range_failed", "range" => &range.raw_line);
            let err_text = apply_premium_to_md(&tf(
                "studio.trim.error.trim_failed",
                &[("range", &md_escape(&range.raw_line))],
            ));
            let _ = send_text_md(api, chat_id, &err_text).await;
            continue;
        }

        // Check file size limit (2GB Bot API ceiling)
        let file_size = std::fs::metadata(&output_path).map(|m| m.len()).unwrap_or(0);
        if file_size > 2000 * 1024 * 1024 {
            log_ev!("studio_trim", trace_id, "file_oversized", "size" => file_size);
            let over_text = apply_premium_to_md(&tf(
                "studio.trim.error.oversized",
                &[("range", &md_escape(&range.raw_line))],
            ));
            let _ = send_text_md(api, chat_id, &over_text).await;
            continue;
        }

        // Caption uses actual clamped timestamps (not raw_line which may have user's unclamped end)
        let caption_range = format!(
            "{} - {}",
            format_timestamp(range.start_secs),
            format_timestamp(range.end_secs)
        );
        let caption = apply_premium_to_md(&format!(
            "{}\n@{}",
            md_escape(&caption_range),
            md_escape(&bot_username)
        ));

        let clip_duration = range.end_secs.saturating_sub(range.start_secs);
        let thumb_path = work_dir.join(format!("thumb_{}_{}.jpg", idx + 1, trace_id));

        let upload_start = Instant::now();
        let mut send_video_params = SendVideoParams::builder()
            .chat_id(chat_id)
            .video(FileUpload::InputFile(InputFile {
                path: output_path.clone(),
            }))
            .caption(&caption)
            .parse_mode(ParseMode::MarkdownV2)
            .supports_streaming(true)
            .build();

        if let Ok(out_meta) = run_ffprobe(&output_path) {
            if out_meta.width > 0 {
                send_video_params.width = Some(out_meta.width as u32);
            }
            if out_meta.height > 0 {
                send_video_params.height = Some(out_meta.height as u32);
            }
        }
        if clip_duration > 0 {
            send_video_params.duration = Some(clip_duration as u32);
        }
        if thumb_path.exists() && std::fs::metadata(&thumb_path).map(|m| m.len() > 0).unwrap_or(false) {
            send_video_params.thumbnail = Some(FileUpload::InputFile(InputFile {
                path: thumb_path,
            }));
        }

        use crate::bot::send_file_with_upload_ticker;
        match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
            api,
            "sendVideo",
            &send_video_params,
            &output_path,
            chat_id,
            status_msg_id,
            "transfer.stage.sending_video",
            None,
        ).await {
            Ok(_) => {
                let up_secs = upload_start.elapsed().as_secs();
                let up_elapsed = upload_start.elapsed();
                let up_speed = if up_elapsed.as_secs_f64() > 0.0 {
                    file_size as f64 / up_elapsed.as_secs_f64()
                } else {
                    0.0
                };
                if let Some(jid) = stats_job_id {
                    crate::stats::record_upload_done(
                        jid,
                        user_id,
                        file_size as i64,
                        Some(up_speed as i64),
                        Some((idx + 1) as i32),
                    )
                    .await;
                }
                total_upload_secs += up_secs;
                log_ev!("studio_trim", trace_id, "trim_delivered", "range" => &range.raw_line, "upload_secs" => up_secs);
                if let Some(ref db) = database {
                    let first_up = now_epoch();
                    let _ = add_traffic(db.client(), user_id, file_size as i64, first_up).await;
                    record_event_user(user_id, "studio_trim", "trim", "ok", 1).await;
                }
            }
            Err(e) => {
                log_ev!("studio_trim", trace_id, "send_video_failed", "=>" => format!("fail err={e}"));
            }
        }


        // Update ticker to next cut for outer task
        current_cut.store(idx + 2, Ordering::Relaxed);
    }

    stop_ticker.store(true, Ordering::Relaxed);
    let loop_secs = loop_start.elapsed().as_secs();
    let trim_secs = loop_secs.saturating_sub(total_upload_secs);
    let upload_secs = total_upload_secs;

    release_cpu(cores, trace_id).await;
    trim_memory();
    remove_active_job(user_id);

    // Delete ticker message
    let _ = api.delete_message(&DeleteMessageParams::builder().chat_id(chat_id).message_id(status_msg_id).build()).await;

    if cancel_flag.load(Ordering::Relaxed) {
        let _ = send_text_md(api, chat_id, &t("studio.trim.job_cancelled")).await;
    } else {
        // Summary message (0.5s after last video)
        tokio::time::sleep(Duration::from_millis(500)).await;

        let fmt_dur = |s: u64| -> String {
            let m = s / 60;
            let sec = s % 60;
            if m > 0 {
                format!("{m} دقیقه و {sec} ثانیه")
            } else {
                format!("{sec} ثانیه")
            }
        };
        let raw_summary = tf(
            "studio.trim.job_done",
            &[
                ("trim_time", &md_escape(&fmt_dur(trim_secs))),
                ("download_time", &md_escape(&fmt_dur(download_secs))),
                ("upload_time", &md_escape(&fmt_dur(upload_secs))),
            ],
        );
        let summary_text = apply_premium_to_md(&raw_summary);
        let _ = send_text_md(api, chat_id, &summary_text).await;

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Re-arm user back to AwaitingStudioTrimRanges state
        flow_manager.set(
            user_id,
            FlowState::AwaitingStudioTrimRanges {
                file_id: file_id.to_string(),
                filename: filename.to_string(),
                duration_secs,
            },
        );

        let rearm_text = apply_premium_to_md(&t("studio.trim.ranges_prompt"));
        let _ = send_text_md_with_keyboard(api, chat_id, &rearm_text, cancel_keyboard()).await;
    }
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_digits() {
        assert_eq!(normalize_digits("۰۰:۰۱:۳۰ - ۰۰:۰۵:۴۵"), "00:01:30 - 00:05:45");
        assert_eq!(normalize_digits("٠٠:٠١:٣٠ - ٠٠:٠٥:٤٥"), "00:01:30 - 00:05:45");
        assert_eq!(normalize_digits("00:01:00 - 00:02:00"), "00:01:00 - 00:02:00");
    }

    #[test]
    fn test_parse_timestamp() {
        assert_eq!(parse_timestamp("01:30"), Some(90));
        assert_eq!(parse_timestamp("01:02:03"), Some(3723));
        assert_eq!(parse_timestamp("00:00:00"), Some(0));
        assert_eq!(parse_timestamp("00:60"), None);
        assert_eq!(parse_timestamp("invalid"), None);
    }

    #[test]
    fn test_parse_cut_ranges_valid() {
        let input = "00:00 - 00:30\n00:01:00-00:02:00\n\n۰۰:۰۲:۳۰ - ۰۰:۰۳:۰۰";
        let duration = 300;
        let res = parse_cut_ranges(input, duration, 10);
        assert!(res.is_ok());
        let ranges = res.unwrap();
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0].start_secs, 0);
        assert_eq!(ranges[0].end_secs, 30);
        assert_eq!(ranges[1].start_secs, 60);
        assert_eq!(ranges[1].end_secs, 120);
        assert_eq!(ranges[2].start_secs, 150);
        assert_eq!(ranges[2].end_secs, 180);
    }

    #[test]
    fn test_parse_cut_ranges_invalid_bounds() {
        let input = "00:02:00 - 00:01:00\n00:06:00 - 00:10:00";
        let duration = 300; // 5 mins
        let res = parse_cut_ranges(input, duration, 10);
        assert!(res.is_err());
        let errors = res.unwrap_err();
        assert_eq!(errors.len(), 2);
        assert!(matches!(errors[0], RangeError::StartGteEnd { .. }));
        assert!(matches!(errors[1], RangeError::EndExceedsDuration { .. }));
    }

    #[test]
    fn test_parse_cut_ranges_autoclamp() {
        let input = "00:01:00 - 00:10:00";
        let duration = 300; // 5 mins
        let res = parse_cut_ranges(input, duration, 10);
        assert!(res.is_ok());
        let ranges = res.unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start_secs, 60);
        assert_eq!(ranges[0].end_secs, 300);
    }

    #[test]
    fn test_parse_cut_ranges_max_cap() {
        let input = "00:01 - 00:02\n00:02 - 00:03\n00:03 - 00:04";
        let res = parse_cut_ranges(input, 300, 2);
        assert!(res.is_err());
        let errors = res.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], RangeError::ExceedsMaxRanges { max: 2 }));
    }

    #[test]
    fn test_parse_cut_ranges_long_text_extraction() {
        let input = "Hello bot! I want to edit this long video for Youtube.\nHere is the description of the video.\nPlease cut the video at the end:\n00:00:00 - 02:00:00\nEnjoy watching!";
        let duration = 7200; // 2 hours
        let res = parse_cut_ranges(input, duration, 10);
        assert!(res.is_ok());
        let ranges = res.unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start_secs, 0);
        assert_eq!(ranges[0].end_secs, 7200);
    }
}
