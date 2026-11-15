use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{DeleteMessageParams, EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, Message, ReplyParameters},
};

use crate::bot::{files::download_telegram_file_metered, messaging::send_text_md};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::panel::btn_icon_danger;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::log::next_trace_id;
use crate::studio::pipeline::{
    TempDirGuard, job_guard, register_active_job, spawn_download_ticker,
};
pub use crate::studio::{TrimMediaKind, detect_trim_media};

use super::probe::{extract_cover_art, format_bitrate, probe_audio_metadata, run_ffprobe};
use super::range::{DEFAULT_MAX_CUT_RANGES, RangeError, format_timestamp, parse_cut_ranges};
use super::runner::execute_trim_job;

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

/// Handles video or audio upload when flow state is `AwaitingStudioTrimVideo`.
pub async fn handle_media_upload(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    let msg_id = message.message_id;

    log_actor_id!("studio_trim", trace_id, user_id, "uploaded" => "media");
    log_ev!("studio_trim", trace_id, "media_received", "user_id" => user_id, "msg_id" => msg_id);

    let media_kind = match detect_trim_media(message) {
        Some(k) => k,
        None => {
            log_ev!("studio_trim", trace_id, "unsupported_media", "=>" => "fail");
            let _ = send_text_md(api, chat_id, &t("studio.trim.error.not_a_video")).await;
            flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
            return;
        }
    };

    let file_id = message
        .video
        .as_ref()
        .map(|v| v.file_id.clone())
        .or_else(|| message.audio.as_ref().map(|a| a.file_id.clone()))
        .or_else(|| message.voice.as_ref().map(|v| v.file_id.clone()))
        .or_else(|| message.document.as_ref().map(|d| d.file_id.clone()));

    let Some(file_id) = file_id else {
        log_ev!("studio_trim", trace_id, "no_file_id", "=>" => "fail");
        let _ = send_text_md(api, chat_id, &t("studio.trim.error.not_a_video")).await;
        flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
        return;
    };

    let orig_filename = message
        .video
        .as_ref()
        .and_then(|v| v.file_name.as_deref())
        .or_else(|| message.audio.as_ref().and_then(|a| a.file_name.as_deref()))
        .or_else(|| {
            message
                .document
                .as_ref()
                .and_then(|d| d.file_name.as_deref())
        })
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            if media_kind == TrimMediaKind::Audio {
                let default_ext = if let Some(audio) = &message.audio {
                    let mime = audio.mime_type.as_deref().unwrap_or("").to_lowercase();
                    if mime.contains("mp4") || mime.contains("m4a") {
                        "m4a"
                    } else if mime.contains("ogg") || mime.contains("opus") {
                        "ogg"
                    } else if mime.contains("flac") {
                        "flac"
                    } else if mime.contains("wav") {
                        "wav"
                    } else if mime.contains("aac") {
                        "aac"
                    } else {
                        "mp3"
                    }
                } else if let Some(doc) = &message.document {
                    let mime = doc.mime_type.as_deref().unwrap_or("").to_lowercase();
                    if mime.contains("mp4") || mime.contains("m4a") {
                        "m4a"
                    } else if mime.contains("ogg") || mime.contains("opus") {
                        "ogg"
                    } else if mime.contains("flac") {
                        "flac"
                    } else if mime.contains("wav") {
                        "wav"
                    } else if mime.contains("aac") {
                        "aac"
                    } else {
                        "mp3"
                    }
                } else if message.voice.is_some() {
                    "ogg"
                } else {
                    "mp3"
                };

                if let Some(audio) = &message.audio {
                    if let Some(title) = &audio.title {
                        let artist = audio.performer.as_deref().unwrap_or("");
                        let name = crate::common::format::safe_music_filename(artist, title);
                        if !name.is_empty() {
                            return format!("{name}.{default_ext}");
                        }
                    }
                }
                if message.voice.is_some() {
                    "voice.ogg".to_string()
                } else {
                    format!("audio.{default_ext}")
                }
            } else {
                "video.mp4".to_string()
            }
        });

    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_active_job(user_id, cancel_flag.clone());
    let _job_guard = job_guard(user_id);

    // 1. Reply with initial status: "در حال دانلود ویدئو/مدیا..."
    let status_raw = tf(
        "studio.trim.status_downloading",
        &[("elapsed", &md_escape("0s")), ("detail", "")],
    );
    let status_text = apply_premium_to_md(&status_raw);
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .reply_parameters(ReplyParameters::builder().message_id(msg_id).build())
        .text(&status_text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(
            job_cancel_keyboard(),
        ))
        .build();

    let status_msg_id = match api.send_message(&params).await {
        Ok(resp) => resp.result.message_id,
        Err(e) => {
            log_ev!("studio_trim", trace_id, "status_send_failed", "=>" => format!("fail err={e}"));
            flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
            return;
        }
    };

    // Download file
    let work_dir = std::env::temp_dir().join(format!("studio_trim_{trace_id}_{user_id}"));
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        log_ev!("studio_trim", trace_id, "mkdir_failed", "=>" => format!("fail err={e}"));
        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .build(),
            )
            .await;
        let _ = send_text_md(api, chat_id, &t("studio.trim.error.download_failed")).await;
        flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
        return;
    }
    let _guard = TempDirGuard::new(work_dir.clone());
    let total_bytes = message
        .video
        .as_ref()
        .and_then(|v| v.file_size)
        .or_else(|| message.audio.as_ref().and_then(|a| a.file_size))
        .or_else(|| message.voice.as_ref().and_then(|v| v.file_size))
        .or_else(|| message.document.as_ref().and_then(|d| d.file_size))
        .unwrap_or(0);

    let local_file = work_dir.join(&orig_filename);

    let dl_stop_flag = spawn_download_ticker(
        api.clone(),
        chat_id,
        status_msg_id,
        local_file.clone(),
        total_bytes,
        "studio.trim",
        Some(cancel_flag.clone()),
    );

    let dl_res =
        download_telegram_file_metered(api, &file_id, &local_file, None, Some(cancel_flag.clone()))
            .await;
    dl_stop_flag.store(true, Ordering::Relaxed);

    if cancel_flag.load(Ordering::Relaxed) {
        log_ev!("studio_trim", trace_id, "download_cancelled", "user_id" => user_id);
        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .build(),
            )
            .await;
        let _ = send_text_md(api, chat_id, &t("studio.trim.cancelled")).await;
        flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
        return;
    }

    if let Err(e) = dl_res {
        log_ev!("studio_trim", trace_id, "download_failed", "=>" => format!("fail err={e}"));
        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .build(),
            )
            .await;
        let _ = send_text_md(api, chat_id, &t("studio.trim.error.download_failed")).await;
        flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
        return;
    }

    if cancel_flag.load(Ordering::Relaxed) {
        log_ev!("studio_trim", trace_id, "probe_cancelled", "user_id" => user_id);
        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .build(),
            )
            .await;
        let _ = send_text_md(api, chat_id, &t("studio.trim.cancelled")).await;
        flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
        return;
    }

    // Edit status to "در حال پردازش..."
    let processing_text = apply_premium_to_md(&t("studio.trim.status_processing"));
    let _ = api
        .edit_message_text(
            &EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(status_msg_id)
                .text(&processing_text)
                .parse_mode(ParseMode::MarkdownV2)
                .build(),
        )
        .await;

    match media_kind {
        TrimMediaKind::Audio => {
            let meta = match probe_audio_metadata(&local_file).await {
                Ok(m) => m,
                Err(e) => {
                    log_ev!("studio_trim", trace_id, "audio_probe_failed", "=>" => format!("fail err={e}"));
                    let _ = api
                        .delete_message(
                            &DeleteMessageParams::builder()
                                .chat_id(chat_id)
                                .message_id(status_msg_id)
                                .build(),
                        )
                        .await;
                    let _ =
                        send_text_md(api, chat_id, &t("studio.trim.error.ffprobe_failed")).await;
                    flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
                    return;
                }
            };

            // Pre-extract cover art thumbnail if present
            let test_cover = work_dir.join("cover_test.jpg");
            let _ = extract_cover_art(&local_file, &test_cover).await;

            let duration_formatted = format_timestamp(meta.duration_secs);
            let bitrate_formatted = format_bitrate(meta.bitrate);

            let effective_filename = if !orig_filename.contains('.') {
                format!("{orig_filename}.{}", meta.ext)
            } else {
                orig_filename.clone()
            };

            let raw_info = tf(
                "studio.trim.info_audio_template",
                &[
                    ("filename", &md_escape(&effective_filename)),
                    ("duration", &md_escape(&duration_formatted)),
                    ("bitrate", &md_escape(&bitrate_formatted)),
                    ("codec", &md_escape(&meta.codec)),
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
                    log_ev!("studio_trim", trace_id, "audio_meta_sent", "duration" => meta.duration_secs);
                    flow_manager.set(
                        user_id,
                        FlowState::AwaitingStudioTrimRanges {
                            file_id,
                            filename: effective_filename,
                            duration_secs: meta.duration_secs,
                        },
                    );
                }
                Err(e) => {
                    log_ev!("studio_trim", trace_id, "send_audio_meta_failed", "=>" => format!("fail err={e}"));
                    let _ =
                        send_text_md(api, chat_id, &t("studio.trim.error.ffprobe_failed")).await;
                    flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
                }
            }
        }
        TrimMediaKind::Video => {
            let meta = match run_ffprobe(&local_file).await {
                Ok(m) => m,
                Err(e) => {
                    log_ev!("studio_trim", trace_id, "ffprobe_failed", "=>" => format!("fail err={e}"));
                    let _ = api
                        .delete_message(
                            &DeleteMessageParams::builder()
                                .chat_id(chat_id)
                                .message_id(status_msg_id)
                                .build(),
                        )
                        .await;
                    let _ =
                        send_text_md(api, chat_id, &t("studio.trim.error.ffprobe_failed")).await;
                    flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
                    return;
                }
            };

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
                    let _ =
                        send_text_md(api, chat_id, &t("studio.trim.error.ffprobe_failed")).await;
                    flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
                }
            }
        }
    }
}

/// Convenience alias maintaining backwards compatibility with callers.
pub async fn handle_video_upload(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    handle_media_upload(api, message, user_id, flow_manager).await;
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
                            &[("line", &line_idx.to_string()), ("text", &md_escape(&text))],
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
