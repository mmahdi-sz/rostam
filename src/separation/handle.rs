use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::EditMessageTextParams,
    types::Message,
};

use crate::bot::{edit_to_ai_lab, send_text_with_ai_back};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{entities_for_text, t};
use crate::log::next_trace_id;

#[allow(unused_imports)]
pub use super::keyboards::{CB_AI_SEP, CB_SEP_BACK, CB_SEP_PREFIX, CB_SEP_QUEUE_CANCEL};
use super::client::fetch_cpu_status;
use super::format::delete_message;
use super::keyboards::{mode_keyboard, prompt_keyboard, queue_cancel_keyboard};
use super::log_trace;
use super::media::{download_file, extract_and_prepare_audio};
use super::quota::{check_and_reserve_quota, probe_audio_duration};
use super::runner::{run_separation_task, SeparationTaskParams};
use super::types::SeparationMode;

/// Called when user presses AI Lab -> Audio Separation.
pub async fn enter_separation(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.set(user_id, FlowState::AwaitingSeparation);
    log_actor_id!("sep", trace_id, user_id, "clicked" => "ai:sep");
    log_ev!("sep", trace_id, "enter", "user_id" => user_id, "chat_id" => chat_id);

    let text = t("separation.send_audio_prompt");
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .reply_markup(prompt_keyboard(message_id))
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    match api.edit_message_text(&params).await {
        Ok(_) => log_trace(trace_id, "prompt_shown", ""),
        Err(e) => log_trace(trace_id, "prompt_failed", &format!("=> fail err={e}")),
    }
}

/// Called when user sends audio while in `AwaitingSeparation` state.
pub async fn handle_separation_audio(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    let msg_id = message.message_id;

    log_actor_id!("sep", trace_id, user_id, "clicked" => "audio/voice");
    log_trace(
        trace_id,
        "audio_received",
        &format!(
            "user_id={user_id} chat_id={chat_id} msg_id={msg_id} has_audio={} has_voice={} has_doc={}",
            message.audio.is_some(),
            message.voice.is_some(),
            message.document.is_some()
        ),
    );

    // Keep flow alive — mode hasn't been selected yet.
    // We store the file_id so we can download after mode selection.
    let is_video = message.video.is_some();
    let file_id = message
        .audio
        .as_ref()
        .map(|a| a.file_id.clone())
        .or_else(|| message.voice.as_ref().map(|v| v.file_id.clone()))
        .or_else(|| message.video.as_ref().map(|v| v.file_id.clone()))
        .or_else(|| message.document.as_ref().map(|d| d.file_id.clone()));

    let Some(file_id) = file_id else {
        log_trace(trace_id, "no_file_id", "");
        let _ = send_text_with_ai_back(api, chat_id, &t("separation.error.invalid_audio")).await;
        return;
    };

    let orig_filename = message
        .audio
        .as_ref()
        .and_then(|a| a.file_name.as_deref())
        .or_else(|| {
            message
                .document
                .as_ref()
                .and_then(|d| d.file_name.as_deref())
        })
        .unwrap_or(if is_video { "video.mp4" } else { "audio.mp3" })
        .to_string();

    log_trace(
        trace_id,
        "file_stored",
        &format!("file_id={file_id} filename={orig_filename} is_video={is_video}"),
    );

    // Update flow to store file info, waiting for mode selection.
    flow_manager.set(
        user_id,
        FlowState::AwaitingSeparationMode {
            file_id: file_id.clone(),
            filename: orig_filename.clone(),
            prompt_msg_id: None,
            is_video,
        },
    );

    // Send mode selection keyboard as a new message.
    let text = t("separation.select_mode");
    let kb = mode_keyboard(msg_id);
    let params = frankenstein::methods::SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(kb))
        .build();
    match api.send_message(&params).await {
        Ok(resp) => {
            let prompt_id = resp.result.message_id;
            log_trace(
                trace_id,
                "mode_keyboard_sent",
                &format!("prompt_msg_id={prompt_id}"),
            );
            // Store the prompt message id so we can edit/delete it later.
            flow_manager.set(
                user_id,
                FlowState::AwaitingSeparationMode {
                    file_id,
                    filename: orig_filename,
                    prompt_msg_id: Some(prompt_id),
                    is_video,
                },
            );
        }
        Err(e) => log_trace(
            trace_id,
            "mode_keyboard_failed",
            &format!("=> fail err={e}"),
        ),
    }
}

/// Direct entry into separation mode selection from inline buttons under audio messages.
pub async fn handle_direct_separation(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    file_id: &str,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_actor_id!("sep", trace_id, user_id, "clicked" => "sep:direct");
    log_ev!("sep", trace_id, "direct_enter", "file_id" => file_id);

    let filename = "audio.mp3".to_string();

    let text = t("separation.select_mode");
    let kb = mode_keyboard(0);
    let params = frankenstein::methods::SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(kb))
        .build();

    match api.send_message(&params).await {
        Ok(resp) => {
            let prompt_id = resp.result.message_id;
            flow_manager.set(
                user_id,
                FlowState::AwaitingSeparationMode {
                    file_id: file_id.to_string(),
                    filename,
                    prompt_msg_id: Some(prompt_id),
                    is_video: false,
                },
            );
        }
        Err(e) => log_trace(
            trace_id,
            "direct_mode_keyboard_failed",
            &format!("=> fail err={e}"),
        ),
    }
}

/// Handles all `sep:*` callbacks.
pub async fn handle_separation_callback(
    api: &Bot,
    cb_data: &str,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    log_trace(
        trace_id,
        "callback",
        &format!("user_id={user_id} chat_id={chat_id} data={cb_data}"),
    );

    // sep:qcancel — user cancelled while in queue
    if cb_data == CB_SEP_QUEUE_CANCEL {
        log_trace(trace_id, "queue_cancel", &format!("user_id={user_id}"));
        if let FlowState::AwaitingSeparationQueued { cancel } = flow_manager.get(user_id) {
            cancel.store(true, Ordering::Relaxed);
        }
        flow_manager.clear(user_id);
        let r = edit_to_ai_lab(api, chat_id, message_id).await;
        log_trace(trace_id, "queue_cancel_done", &format!("ok={}", r.is_ok()));
        return;
    }

    // sep:back:{msg_id} — back to AI Lab from prompt page
    if cb_data.starts_with("sep:back:") {
        flow_manager.clear(user_id);
        let r = edit_to_ai_lab(api, chat_id, message_id).await;
        log_trace(trace_id, "back_done", &format!("ok={}", r.is_ok()));
        return;
    }

    // sep:cancel:{msg_id}
    if let Some(rest) = cb_data.strip_prefix("sep:cancel:") {
        log_trace(trace_id, "cancel", &format!("msg_id_from_cb={rest}"));
        flow_manager.clear(user_id);
        let r = edit_to_ai_lab(api, chat_id, message_id).await;
        log_trace(trace_id, "cancel_done", &format!("ok={}", r.is_ok()));
        return;
    }

    // sep:quality:{orig_msg_id} or sep:fast:{orig_msg_id}
    let (mode, _orig_msg_id) = if let Some(rest) = cb_data.strip_prefix("sep:quality:") {
        (SeparationMode::Quality, rest)
    } else if let Some(rest) = cb_data.strip_prefix("sep:fast:") {
        (SeparationMode::Fast, rest)
    } else {
        log_trace(trace_id, "unknown_callback", &format!("data={cb_data}"));
        return;
    };

    let mode_label = match mode {
        SeparationMode::Quality => "quality",
        SeparationMode::Fast => "fast",
    };
    log_trace(
        trace_id,
        "mode_selected",
        &format!("user_id={user_id} mode={mode_label}"),
    );

    // Read stored file info from flow state.
    let (file_id, filename, is_video) = match flow_manager.get(user_id) {
        FlowState::AwaitingSeparationMode {
            file_id,
            filename,
            is_video,
            ..
        } => (file_id, filename, is_video),
        other => {
            log_trace(trace_id, "wrong_state", &format!("state={other:?}"));
            let _ =
                send_text_with_ai_back(api, chat_id, &t("separation.error.service_unavailable"))
                    .await;
            return;
        }
    };

    if crate::moebius::cpu::is_user_cpu_busy(user_id).await {
        let _ = send_text_with_ai_back(api, chat_id, &t("active_job_running")).await;
        return;
    }

    // Clear flow — processing starts.
    flow_manager.clear(user_id);

    // Edit keyboard message to "processing…"
    let processing_text = if is_video {
        t("separation.extracting_audio")
    } else {
        t("separation.processing")
    };
    let edit_params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&processing_text)
        .build();
    match api.edit_message_text(&edit_params).await {
        Ok(_) => log_trace(trace_id, "processing_msg_shown", "is_video={is_video}"),
        Err(e) => log_trace(trace_id, "processing_msg_failed", &format!("err={e}")),
    }

    let stats_job_id = crate::stats::record_download_start(user_id, "separation").await;

    // Download file from Telegram.
    log_trace(
        trace_id,
        "download_start",
        &format!("file_id={file_id} filename={filename} is_video={is_video}"),
    );
    let (file_bytes, dl_result) = match download_file(api, &file_id, trace_id).await {
        Ok(res) => res,
        Err(e) => {
            log_trace(trace_id, "download_failed", &format!("err={e}"));
            crate::stats::record_event_user(user_id, "separation", mode_label, "fail", 0).await;
            crate::stats::record_error_global("separation", &format!("download failed: {e}")).await;
            let _ =
                send_text_with_ai_back(api, chat_id, &t("separation.error.service_unavailable"))
                    .await;
            let _ = delete_message(api, chat_id, message_id).await;
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

    log_trace(
        trace_id,
        "download_done",
        &format!("bytes={}", file_bytes.len()),
    );

    // If video: extract audio with ffmpeg, then compress if needed.
    let tmp_dir = std::env::temp_dir().join(format!("sep_{trace_id}"));
    std::fs::create_dir_all(&tmp_dir).ok();

    let audio_bytes = if is_video {
        match extract_and_prepare_audio(&file_bytes, &tmp_dir, message_id, chat_id, api, trace_id)
            .await
        {
            Ok(b) => b,
            Err(e) => {
                log_trace(trace_id, "extract_failed", &format!("err={e}"));
                crate::stats::record_event_user(user_id, "separation", mode_label, "fail", 0).await;
                crate::stats::record_error_global(
                    "separation",
                    &format!("audio extraction failed: {e}"),
                )
                .await;
                let _ = send_text_with_ai_back(
                    api,
                    chat_id,
                    &t("separation.error.audio_extraction_failed"),
                )
                .await;
                let _ = delete_message(api, chat_id, message_id).await;
                std::fs::remove_dir_all(&tmp_dir).ok();
                return;
            }
        }
    } else {
        file_bytes
    };
    log_trace(
        trace_id,
        "audio_ready",
        &format!("bytes={}", audio_bytes.len()),
    );

    // Quota reservation
    let audio_duration_secs = probe_audio_duration(&tmp_dir, &audio_bytes, trace_id).await;
    let quota_res = match check_and_reserve_quota(
        api,
        chat_id,
        message_id,
        user_id,
        database,
        flow_manager,
        &tmp_dir,
        audio_duration_secs,
        trace_id,
    )
    .await
    {
        Ok(res) => res,
        Err(()) => return,
    };

    // Update status to processing.
    if is_video {
        let edit_params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(t("separation.processing"))
            .build();
        let _ = api.edit_message_text(&edit_params).await;
    }

    // Call separation service.
    log_trace(trace_id, "separate_start", &format!("mode={mode_label}"));
    let audio_filename: String = if is_video {
        "audio.mp3".to_string()
    } else {
        filename.clone()
    };

    // Check server load before showing any message.
    let cpu_status = fetch_cpu_status().await;
    let server_free = cpu_status.available_cores > 0 && !cpu_status.overloaded;
    log_trace(
        trace_id,
        "cpu_status_check",
        &format!(
            "available={} overloaded={} queue={} server_free={server_free}",
            cpu_status.available_cores, cpu_status.overloaded, cpu_status.queue_length
        ),
    );
    if !server_free {
        crate::stats::record_event_user(user_id, "cpu", "queue", "separation", 0).await;
    }

    // Cancel token: set to true if user presses cancel while in queue.
    let cancel_flag = Arc::new(AtomicBool::new(false));
    flow_manager.set(
        user_id,
        FlowState::AwaitingSeparationQueued {
            cancel: cancel_flag.clone(),
        },
    );

    // Show initial message.
    {
        let text = if server_free {
            t("separation.processing_queued")
        } else {
            t("separation.queue.waiting")
        };
        let entities = entities_for_text(&text);
        let kb = queue_cancel_keyboard();
        let mut params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(&text)
            .reply_markup(kb)
            .build();
        if !entities.is_empty() {
            params.entities = Some(entities);
        }
        let _ = api.edit_message_text(&params).await;
        log_trace(
            trace_id,
            "initial_msg_shown",
            &format!("server_free={server_free}"),
        );
    }

    // Spawn all heavy work so the event loop stays free — this makes the cancel button work.
    let task_params = SeparationTaskParams {
        api: api.clone(),
        chat_id,
        message_id,
        user_id,
        database: database.clone(),
        flow_manager: flow_manager.clone(),
        audio_bytes,
        audio_filename,
        mode,
        mode_label,
        reserved: quota_res.reserved,
        reserve_secs: quota_res.reserve_secs,
        audio_duration_secs,
        tmp_dir,
        cancel_flag,
        stats_job_id,
        trace_id,
    };
    crate::app::spawn_user_task(run_separation_task(task_params));
}
