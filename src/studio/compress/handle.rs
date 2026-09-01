use std::sync::atomic::Ordering;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, Message},
};

use super::runner::start_compression_job;
use super::session::{CompressSession, clear_session, load_session, save_session};
use super::ui::{build_compress_keyboard, build_compress_text};
use crate::bot::constants::{
    CB_STUDIO_COMPRESS, CB_STUDIO_COMPRESS_CANCEL, CB_STUDIO_COMPRESS_JOBCANCEL,
    CB_STUDIO_COMPRESS_START,
};
use crate::emoji::panel::btn_icon_danger;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::log::next_trace_id;
use crate::studio::pipeline::{TempDirGuard, cancel_active_job};
use crate::studio::trim::run_ffprobe;

pub async fn enter_compress_prompt(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.set(user_id, FlowState::AwaitingStudioCompressVideo);
    log_actor_id!("studio_compress", trace_id, user_id, "clicked" => CB_STUDIO_COMPRESS);

    let text = apply_premium_to_md(&t("studio.compress.send_video_prompt"));
    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("studio.compress.cancel_btn"),
            CB_STUDIO_COMPRESS_CANCEL,
            "cancel",
        )]])
        .build();

    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(kb)
        .build();

    let _ = api.edit_message_text(&params).await;
}

pub async fn handle_video_upload(
    api: &Bot,
    msg: Message,
    user_id: i64,
    trace_id: u64,
    flow_manager: &FlowManager,
) {
    let chat_id = msg.chat.id;

    if !crate::studio::is_video_message_metadata(&msg) {
        log_ev!("studio_compress", trace_id, "not_a_video_metadata", "=>" => "fail");
        let _ =
            crate::bot::send_text_md(api, chat_id, &t("studio.compress.error.not_a_video")).await;
        return;
    }

    let (file_id, raw_filename, orig_size_bytes) = if let Some(v) = msg.video.as_ref() {
        (
            v.file_id.clone(),
            v.file_name.as_deref().unwrap_or("video.mp4").to_string(),
            v.file_size.unwrap_or(0),
        )
    } else if let Some(d) = msg.document.as_ref() {
        (
            d.file_id.clone(),
            d.file_name.as_deref().unwrap_or("video.mp4").to_string(),
            d.file_size.unwrap_or(0),
        )
    } else {
        log_ev!("studio_compress", trace_id, "invalid_video_msg", "=>" => "fail");
        let _ =
            crate::bot::send_text_md(api, chat_id, &t("studio.compress.error.not_a_video")).await;
        return;
    };

    log_actor_id!("studio_compress", trace_id, user_id, "uploaded" => "video");
    let file_id_prefix = if file_id.len() >= 6 {
        &file_id[..6]
    } else {
        &file_id
    };
    log_ev!("studio_compress", trace_id, "video_received", "user_id" => user_id, "file_id" => file_id_prefix);

    let status_raw = tf(
        "studio.compress.status_downloading",
        &[("elapsed", &md_escape("0s")), ("detail", "")],
    );
    let status_text = apply_premium_to_md(&status_raw);
    let status_msg = match api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&status_text)
                .parse_mode(ParseMode::MarkdownV2)
                .build(),
        )
        .await
    {
        Ok(m) => m,
        Err(e) => {
            log_ev!("studio_compress", trace_id, "status_send_failed", "=>" => format!("fail err={e}"));
            return;
        }
    };

    let work_dir = std::env::temp_dir().join(format!("studio_compress_{trace_id}_{user_id}"));
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        log_ev!("studio_compress", trace_id, "mkdir_failed", "=>" => format!("fail err={e}"));
        let _ = crate::bot::send_text_md(api, chat_id, &t("studio.compress.error.download_failed"))
            .await;
        return;
    }
    let _guard = TempDirGuard::new(work_dir.clone());

    let local_file = work_dir.join(&raw_filename);

    let dl_stop_flag = crate::studio::pipeline::spawn_download_ticker(
        api.clone(),
        chat_id,
        status_msg.result.message_id,
        local_file.clone(),
        orig_size_bytes,
        "studio.compress",
        None,
    );

    let dl_res = crate::bot::files::download_telegram_file(api, &file_id, &local_file).await;
    dl_stop_flag.store(true, Ordering::Relaxed);

    if let Err(e) = dl_res {
        log_ev!("studio_compress", trace_id, "download_failed", "=>" => format!("fail err={e}"));
        let _ = crate::bot::send_text_md(api, chat_id, &t("studio.compress.error.download_failed"))
            .await;
        return;
    }

    let meta = match run_ffprobe(&local_file).await {
        Ok(m) => m,
        Err(e) => {
            log_ev!("studio_compress", trace_id, "ffprobe_failed", "=>" => format!("fail err={e}"));
            let _ =
                crate::bot::send_text_md(api, chat_id, &t("studio.compress.error.ffprobe_failed"))
                    .await;
            return;
        }
    };

    // Determine initial defaults
    let orig_w = meta.width.max(1);
    let orig_h = meta.height.max(1);
    let orig_fps = meta.fps.max(1);
    let orig_bitrate = meta.bitrate.max(100_000);
    let duration_secs = meta.duration_secs.max(1);

    // Initial selected resolution scale is base_dim or nearest lower standard height
    let base_dim = orig_w.min(orig_h);
    let initial_res_h = [2160, 1440, 1080, 720, 480, 360, 240, 144]
        .into_iter()
        .find(|&h| h <= base_dim)
        .unwrap_or(base_dim);

    // Initial selected FPS is orig_fps or nearest lower standard FPS
    let initial_fps = [60, 45, 30, 24, 20, 15, 13]
        .into_iter()
        .find(|&f| f <= orig_fps)
        .unwrap_or(orig_fps);

    let initial_codec = match meta.codec.to_lowercase().as_str() {
        "av1" | "svtav1" | "libsvtav1" => "av1",
        "h265" | "hevc" | "libx265" => "h265",
        "vp9" | "libvpx-vp9" => "vp9",
        _ => "h264",
    }
    .to_string();

    let session = CompressSession {
        file_id: file_id.clone(),
        filename: raw_filename.to_string(),
        orig_w,
        orig_h,
        orig_fps,
        orig_bitrate,
        orig_codec: meta.codec.clone(),
        orig_size_bytes,
        duration_secs,

        codec: initial_codec,
        res_h: initial_res_h,
        fps: initial_fps,
        br_ratio: 100,
    };

    save_session(user_id, &session).await;
    flow_manager.clear(user_id);

    let menu_text = build_compress_text(&session);
    let menu_kb = build_compress_keyboard(&session);

    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(status_msg.result.message_id)
        .text(&menu_text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(menu_kb)
        .build();

    let _ = api.edit_message_text(&params).await;
}

pub async fn handle_compress_cb(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    cb_data: &str,
    flow_manager: &FlowManager,
) -> bool {
    let trace_id = next_trace_id();
    log_ev!("studio_compress", trace_id, "callback", "cb" => cb_data, "user_id" => user_id);

    if cb_data == CB_STUDIO_COMPRESS_CANCEL {
        clear_session(user_id).await;
        flow_manager.clear(user_id);
        crate::studio::enter_studio(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data == CB_STUDIO_COMPRESS_JOBCANCEL {
        let cancelled = cancel_active_job(user_id);
        log_ev!("studio_compress", trace_id, "job_cancel_result", "cancelled" => cancelled);
        true
    } else if cb_data == CB_STUDIO_COMPRESS_START {
        let Some(session) = load_session(user_id).await else {
            let _ =
                crate::bot::send_text_md(api, chat_id, &t("studio.compress.error.compress_failed"))
                    .await;
            return true;
        };
        start_compression_job(api, chat_id, message_id, user_id, session, flow_manager).await;
        true
    } else if let Some(rest) = cb_data.strip_prefix("stc:set:") {
        let Some(mut session) = load_session(user_id).await else {
            return true;
        };
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() == 2 {
            match parts[0] {
                "c" => session.codec = parts[1].to_string(),
                "r" => {
                    if let Ok(h) = parts[1].parse::<u32>() {
                        let base_dim = session.orig_w.min(session.orig_h);
                        if h <= base_dim {
                            session.res_h = h;
                        }
                    }
                }
                "f" => {
                    if let Ok(f) = parts[1].parse::<u32>() {
                        if f <= session.orig_fps {
                            session.fps = f;
                        }
                    }
                }
                "b" => {
                    if let Ok(b) = parts[1].parse::<u32>() {
                        if [100, 75, 50, 25, 16, 12].contains(&b) {
                            session.br_ratio = b;
                        }
                    }
                }
                _ => {}
            }
            save_session(user_id, &session).await;
            let menu_text = build_compress_text(&session);
            let menu_kb = build_compress_keyboard(&session);

            let params = EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(message_id)
                .text(&menu_text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(menu_kb)
                .build();

            let _ = api.edit_message_text(&params).await;
        }
        true
    } else {
        false
    }
}
