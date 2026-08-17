use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{DeleteMessageParams, EditMessageTextParams, SendAudioParams},
    types::{InlineKeyboardMarkup, Message},
};

use crate::bot::{edit_to_ai_lab, send_text_with_ai_back};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::panel::{btn_icon, btn_icon_danger, btn_icon_primary, btn_icon_success};
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{entities_for_text, t, tf};
use crate::log::next_trace_id;
use crate::rank::{
    self,
    quota::{QuotaKind, get_usage, refund_usage, reserve_usage},
};

use super::client::{fetch_cpu_status, separate_audio};
use super::types::SeparationMode;

// ponytail: thin shim so 30+ log_trace() calls below keep working with correct domain.
fn log_trace(trace_id: u64, event: &str, details: &str) {
    crate::log::emit("sep", trace_id, event, details);
}

#[allow(dead_code)]
pub const CB_AI_SEP: &str = "ai:sep";
pub const CB_SEP_PREFIX: &str = "sep:";
pub const CB_SEP_BACK: &str = "sep:back";
pub const CB_SEP_QUEUE_CANCEL: &str = "sep:qcancel";

fn queue_cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("separation.queue.cancel_btn"),
            CB_SEP_QUEUE_CANCEL,
            "cancel",
        )]])
        .build()
}

fn prompt_keyboard(msg_id: i32) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(
            &t("start.back"),
            &format!("{CB_SEP_BACK}:{msg_id}"),
            "back",
        )]])
        .build()
}

fn mode_keyboard(msg_id: i32) -> InlineKeyboardMarkup {
    // 1 col, 3 rows: High Quality (green) / Fast (blue) / Cancel (red).
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon_success(
                &t("separation.btn.quality"),
                &format!("sep:quality:{msg_id}"),
                "quality_high",
            )],
            vec![btn_icon_primary(
                &t("separation.btn.fast"),
                &format!("sep:fast:{msg_id}"),
                "speed_fast",
            )],
            vec![btn_icon_danger(
                &t("separation.btn.cancel"),
                &format!("sep:cancel:{msg_id}"),
                "cancel",
            )],
        ])
        .build()
}

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
    let mut reserved = false;
    let mut reserve_secs: i64 = 0;
    let audio_duration_secs;
    {
        let tmp_probe = tmp_dir.join("probe_audio");
        std::fs::write(&tmp_probe, &audio_bytes).unwrap_or(());
        audio_duration_secs = if let Some(tmp_probe_str) = tmp_probe.to_str() {
            let probe = tokio::process::Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=duration",
                    "-of",
                    "csv=p=0",
                    tmp_probe_str,
                ])
                .output()
                .await;
            probe
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .and_then(|s| s.trim().parse::<f64>().ok())
                .map(|d| d.ceil() as u64)
                .unwrap_or(0)
        } else {
            0
        };
        std::fs::remove_file(&tmp_probe).ok();
        log_trace(
            trace_id,
            "duration_probed",
            &format!("secs={audio_duration_secs}"),
        );

        if let Some(db) = database.as_ref() {
            let user_rank = rank::effective_rank(db.client(), user_id).await;
            let daily_limit = user_rank.separation_daily_secs();
            let weekly_limit = user_rank.separation_weekly_secs();

            reserve_secs = audio_duration_secs.max(1) as i64;

            macro_rules! deny {
                () => {{
                    let d_used = get_usage(db.client(), user_id, QuotaKind::SeparationDaily, 86400)
                        .await
                        .unwrap_or(0) as u64;
                    let w_used =
                        get_usage(db.client(), user_id, QuotaKind::SeparationWeekly, 7 * 86400)
                            .await
                            .unwrap_or(0) as u64;
                    let d_rem = daily_limit.saturating_sub(d_used);
                    let w_rem = weekly_limit.saturating_sub(w_used);
                    let label = if d_rem == 0 {
                        tf(
                            "separation.quota_daily_limit",
                            &[("limit", &format_duration_fa(daily_limit))],
                        )
                    } else if w_rem == 0 {
                        tf(
                            "separation.quota_weekly_limit",
                            &[("limit", &format_duration_fa(weekly_limit))],
                        )
                    } else {
                        tf(
                            "separation.quota_file_too_long",
                            &[("remaining", &format_duration_fa(d_rem.min(w_rem)))],
                        )
                    };
                    log_trace(
                        trace_id,
                        "quota_blocked",
                        &format!("user_id={user_id} daily_used={d_used} weekly_used={w_used}"),
                    );
                    let _ = delete_message(api, chat_id, message_id).await;
                    std::fs::remove_dir_all(&tmp_dir).ok();
                    flow_manager.clear(user_id);
                    if let Some(min_rank) = user_rank.separation_next_rank() {
                        crate::rank::paywall::block_limit(api, chat_id, &label, min_rank).await;
                    } else {
                        let _ = crate::bot::send_text_with_ai_back(api, chat_id, &label).await;
                    }
                    return;
                }};
            }

            macro_rules! db_fail {
                ($e:expr) => {{
                    log_trace(trace_id, "quota_reserve", &format!("err={} => fail", $e));
                    let _ = delete_message(api, chat_id, message_id).await;
                    std::fs::remove_dir_all(&tmp_dir).ok();
                    flow_manager.clear(user_id);
                    crate::rank::paywall::quota_db_error(
                        api,
                        chat_id,
                        "separation",
                        &format!("{}", $e),
                    )
                    .await;
                    return;
                }};
            }

            match reserve_usage(
                db.client(),
                user_id,
                QuotaKind::SeparationDaily,
                reserve_secs,
                86400,
                daily_limit as i64,
            )
            .await
            {
                Ok(Some(used)) => log_trace(
                    trace_id,
                    "quota_reserved_daily",
                    &format!("used={used} limit={daily_limit}"),
                ),
                Ok(None) => deny!(),
                Err(e) => db_fail!(e),
            }

            match reserve_usage(
                db.client(),
                user_id,
                QuotaKind::SeparationWeekly,
                reserve_secs,
                7 * 86400,
                weekly_limit as i64,
            )
            .await
            {
                Ok(Some(used)) => {
                    reserved = true;
                    log_trace(
                        trace_id,
                        "quota_reserved_weekly",
                        &format!("used={used} limit={weekly_limit}"),
                    );
                }
                Ok(None) => {
                    let _ = refund_usage(
                        db.client(),
                        user_id,
                        QuotaKind::SeparationDaily,
                        reserve_secs,
                        86400,
                    )
                    .await;
                    deny!();
                }
                Err(e) => {
                    let _ = refund_usage(
                        db.client(),
                        user_id,
                        QuotaKind::SeparationDaily,
                        reserve_secs,
                        86400,
                    )
                    .await;
                    db_fail!(e);
                }
            }
        }
    }

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
    let api_task = api.clone();
    let cancel_task = cancel_flag.clone();
    let db_task = database.clone();
    let fm_task = flow_manager.clone();
    crate::app::spawn_user_task(async move {
        macro_rules! refund {
            ($why:expr) => {
                if reserved {
                    if let Some(db) = db_task.as_ref() {
                        log_trace(trace_id, "quota_refund", &format!("why={}", $why));
                        for (kind, window) in [
                            (QuotaKind::SeparationDaily, 86400),
                            (QuotaKind::SeparationWeekly, 7 * 86400),
                        ] {
                            if let Err(e) =
                                refund_usage(db.client(), user_id, kind, reserve_secs, window).await
                            {
                                log_trace(trace_id, "quota_refund", &format!("err={e} => fail"));
                                crate::stats::record_error_global(
                                    "separation",
                                    "quota_refund_failed",
                                )
                                .await;
                            }
                        }
                    }
                }
            };
        }

        let api_status = api_task.clone();
        let cancel_status = cancel_task.clone();
        let eta_total = audio_duration_secs.saturating_mul(match mode {
            SeparationMode::Fast => 3,
            SeparationMode::Quality => 5,
        });
        let status_task = crate::app::spawn_user_task(async move {
            let started = std::time::Instant::now();
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if cancel_status.load(Ordering::Relaxed) {
                    return;
                }
                let elapsed = started.elapsed().as_secs();
                let remaining = eta_total.saturating_sub(elapsed);
                let text = tf(
                    "separation.progress",
                    &[
                        ("elapsed", &format_clock(elapsed)),
                        ("remaining", &format_clock(remaining)),
                    ],
                );
                let entities = entities_for_text(&text);
                let mut params = EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(message_id)
                    .text(&text)
                    .reply_markup(queue_cancel_keyboard())
                    .build();
                if !entities.is_empty() {
                    params.entities = Some(entities);
                }
                let _ = api_status.edit_message_text(&params).await;
            }
        });

        // Race separation against cancel signal (cancel aborts the HTTP request via drop).
        let op_started = std::time::Instant::now();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled2 = cancelled.clone();
        let sep_result = tokio::select! {
            r = tokio::time::timeout(
                Duration::from_secs(35 * 60),
                separate_audio(audio_bytes, &audio_filename, mode, user_id, false),
            ) => {
                match r {
                    Ok(r) => Some(r),
                    Err(_) => None, // 35-min timeout
                }
            }
            _ = async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    if cancel_task.load(Ordering::Relaxed) { break; }
                }
                cancelled2.store(true, Ordering::Relaxed);
            } => { None }
        };

        // Abort orphan status-update task now that we have a result.
        status_task.abort();
        // The whole track sat in RAM twice (read buffer + multipart copy); hand
        // the pages back — separation talks to a remote service, so no broker
        // release happens here to do it.
        crate::moebius::cpu::trim_memory();

        // Clear flow directly here rather than via channel to avoid wiping re-armed state.
        fm_task.clear(user_id);

        if cancelled.load(Ordering::Relaxed) {
            log_trace(trace_id, "cancelled_in_queue", "");
            std::fs::remove_dir_all(&tmp_dir).ok();
            refund!("cancelled_in_queue");
            return;
        }

        let result = match sep_result {
            None => {
                log_trace(trace_id, "queue_timeout", "");
                crate::stats::record_event_user(user_id, "cpu", "timeout", "separation", 0).await;
                crate::stats::record_event_user(user_id, "separation", mode_label, "timeout", 0)
                    .await;
                crate::stats::record_error_global("separation", "queue timeout (35min)").await;
                let _ = crate::bot::send_text_with_ai_back(
                    &api_task,
                    chat_id,
                    &t("separation.error.queue_timeout"),
                )
                .await;
                let _ = delete_message(&api_task, chat_id, message_id).await;
                std::fs::remove_dir_all(&tmp_dir).ok();
                refund!("queue_timeout");
                return;
            }
            Some(r) => r,
        };

        match result {
            Ok(result) => {
                log_trace(
                    trace_id,
                    "separate_done",
                    &format!(
                        "duration={:.1}s vocals_wav={} instrumental_wav={} vocals_compressed={} instrumental_compressed={} ext={}",
                        result.duration_seconds,
                        result.vocals_wav.len(),
                        result.instrumental_wav.len(),
                        result.vocals_compressed.len(),
                        result.instrumental_compressed.len(),
                        result.compressed_ext
                    ),
                );

                let vocals_wav_path = tmp_dir.join("vocals.wav");
                let instrumental_wav_path = tmp_dir.join("instrumental.wav");
                let vocals_compressed_path =
                    tmp_dir.join(format!("vocals.{}", result.compressed_ext));
                let instrumental_compressed_path =
                    tmp_dir.join(format!("instrumental.{}", result.compressed_ext));

                std::fs::write(&vocals_wav_path, &result.vocals_wav).ok();
                std::fs::write(&instrumental_wav_path, &result.instrumental_wav).ok();
                std::fs::write(&vocals_compressed_path, &result.vocals_compressed).ok();
                std::fs::write(
                    &instrumental_compressed_path,
                    &result.instrumental_compressed,
                )
                .ok();

                let up_start = std::time::Instant::now();
                let total_out_bytes = (std::fs::metadata(&vocals_compressed_path)
                    .map(|m| m.len())
                    .unwrap_or(0))
                    + (std::fs::metadata(&vocals_wav_path)
                        .map(|m| m.len())
                        .unwrap_or(0))
                    + (std::fs::metadata(&instrumental_compressed_path)
                        .map(|m| m.len())
                        .unwrap_or(0))
                    + (std::fs::metadata(&instrumental_wav_path)
                        .map(|m| m.len())
                        .unwrap_or(0));

                use crate::bot::send_file_with_upload_ticker;
                let smid = message_id;

                log_trace(trace_id, "send_vocals_compressed", "");
                let p = SendAudioParams::builder()
                    .chat_id(chat_id)
                    .audio(PathBuf::from(&vocals_compressed_path))
                    .caption(t("separation.result.vocals_compressed_caption"))
                    .build();
                match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
                    &api_task,
                    "sendAudio",
                    &p,
                    &vocals_compressed_path,
                    chat_id,
                    smid,
                    "transfer.stage.sending_audio",
                    None,
                )
                .await
                {
                    Ok(_) => log_trace(trace_id, "vocals_compressed_sent", ""),
                    Err(e) => log_trace(trace_id, "vocals_compressed_failed", &format!("err={e}")),
                }

                log_trace(trace_id, "send_vocals_wav", "");
                let p = frankenstein::methods::SendDocumentParams::builder()
                    .chat_id(chat_id)
                    .document(PathBuf::from(&vocals_wav_path))
                    .caption(t("separation.result.vocals_wav_caption"))
                    .build();
                match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
                    &api_task,
                    "sendDocument",
                    &p,
                    &vocals_wav_path,
                    chat_id,
                    smid,
                    "transfer.stage.sending_document",
                    None,
                )
                .await
                {
                    Ok(_) => log_trace(trace_id, "vocals_wav_sent", ""),
                    Err(e) => log_trace(trace_id, "vocals_wav_failed", &format!("err={e}")),
                }

                log_trace(trace_id, "send_instrumental_compressed", "");
                let p = SendAudioParams::builder()
                    .chat_id(chat_id)
                    .audio(PathBuf::from(&instrumental_compressed_path))
                    .caption(t("separation.result.instrumental_compressed_caption"))
                    .build();
                match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
                    &api_task,
                    "sendAudio",
                    &p,
                    &instrumental_compressed_path,
                    chat_id,
                    smid,
                    "transfer.stage.sending_audio",
                    None,
                )
                .await
                {
                    Ok(_) => log_trace(trace_id, "instrumental_compressed_sent", ""),
                    Err(e) => log_trace(
                        trace_id,
                        "instrumental_compressed_failed",
                        &format!("err={e}"),
                    ),
                }

                log_trace(trace_id, "send_instrumental_wav", "");
                let p = frankenstein::methods::SendDocumentParams::builder()
                    .chat_id(chat_id)
                    .document(PathBuf::from(&instrumental_wav_path))
                    .caption(t("separation.result.instrumental_wav_caption"))
                    .build();
                match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
                    &api_task,
                    "sendDocument",
                    &p,
                    &instrumental_wav_path,
                    chat_id,
                    smid,
                    "transfer.stage.sending_document",
                    None,
                )
                .await
                {
                    Ok(_) => log_trace(trace_id, "instrumental_wav_sent", ""),
                    Err(e) => log_trace(trace_id, "instrumental_wav_failed", &format!("err={e}")),
                }

                let up_elapsed = up_start.elapsed();
                let up_speed = if up_elapsed.as_secs_f64() > 0.0 {
                    total_out_bytes as f64 / up_elapsed.as_secs_f64()
                } else {
                    0.0
                };
                if let Some(jid) = stats_job_id {
                    crate::stats::record_upload_done(
                        jid,
                        user_id,
                        total_out_bytes as i64,
                        Some(up_speed as i64),
                        Some(4),
                    )
                    .await;
                }

                // Quota was already reserved via ffprobe duration; no second deduction needed.

                crate::stats::record_event_user(
                    user_id,
                    "separation",
                    mode_label,
                    "ok",
                    result.duration_seconds.ceil() as i64,
                )
                .await;
                crate::metrics::get()
                    .separation_requests_total
                    .with_label_values(&["success"])
                    .inc();

                // Final report and reset flow to separation state for back-to-back audio requests.
                let report = tf(
                    "separation.done_report",
                    &[
                        ("duration", &format_clock(audio_duration_secs)),
                        ("total", &format_clock(op_started.elapsed().as_secs())),
                    ],
                );
                let text = format!("{report}\n\n{}", t("separation.send_audio_prompt"));
                let entities = entities_for_text(&text);
                fm_task.set(user_id, FlowState::AwaitingSeparation);
                let mut params = frankenstein::methods::SendMessageParams::builder()
                    .chat_id(chat_id)
                    .text(&text)
                    .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(
                        prompt_keyboard(0),
                    ))
                    .build();
                if !entities.is_empty() {
                    params.entities = Some(entities);
                }
                let _ = api_task.send_message(&params).await;

                std::fs::remove_dir_all(&tmp_dir).ok();
                log_trace(trace_id, "cleanup_done", "");
            }
            Err(e) => {
                log_trace(trace_id, "separate_error", &format!("err={e}"));
                crate::stats::record_event_user(user_id, "separation", mode_label, "fail", 0).await;
                crate::metrics::get()
                    .separation_requests_total
                    .with_label_values(&["fail"])
                    .inc();
                crate::stats::record_error_global(
                    "separation",
                    &format!("processing error: {e:?}"),
                )
                .await;
                let _ = delete_message(&api_task, chat_id, message_id).await;
                use super::error::SeparationError;
                let key = match &e {
                    SeparationError::ServiceUnavailable => "separation.error.service_unavailable",
                    SeparationError::InvalidAudio => "separation.error.invalid_audio",
                    SeparationError::Timeout => "separation.error.timeout",
                    SeparationError::ProcessingFailed(_) => "separation.error.processing_failed",
                };
                let _ = crate::bot::send_text_with_ai_back(&api_task, chat_id, &t(key)).await;
                std::fs::remove_dir_all(&tmp_dir).ok();
                refund!("separate_error");
            }
        }
    });
    // handle_separation_callback returns immediately; the spawned task does all heavy work.
}

async fn extract_and_prepare_audio(
    video_bytes: &[u8],
    tmp_dir: &std::path::Path,
    message_id: i32,
    chat_id: i64,
    api: &Bot,
    trace_id: u64,
) -> crate::error::Result<Vec<u8>> {
    const MAX_AUDIO_BYTES: u64 = 50 * 1024 * 1024;

    let video_path = tmp_dir.join("input_video");
    std::fs::write(&video_path, video_bytes)?;

    // Extract audio as MP3 at 320kbps.
    let audio_path = tmp_dir.join("extracted.mp3");
    let video_path_str = video_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid UTF-8 path"))?;
    let audio_path_str = audio_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid UTF-8 path"))?;

    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            video_path_str,
            "-vn",
            "-acodec",
            "libmp3lame",
            "-b:a",
            "320k",
            audio_path_str,
        ])
        .output()
        .await?;
    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        anyhow::bail!("ffmpeg extract failed: {stderr}");
    }

    let audio_size = std::fs::metadata(&audio_path)?.len();
    log_trace(trace_id, "audio_extracted", &format!("size={audio_size}"));

    if audio_size <= MAX_AUDIO_BYTES {
        return Ok(std::fs::read(&audio_path)?);
    }

    // Iteratively compress: reduce bitrate by 10% each attempt until < 50MB.
    // Probe current bitrate then step down.
    let probe = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=bit_rate",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            audio_path_str,
        ])
        .output()
        .await?;
    let initial_bitrate: u32 = String::from_utf8_lossy(&probe.stdout)
        .trim()
        .parse()
        .unwrap_or(320_000);

    let mut bitrate_bps = initial_bitrate;
    let mut attempt = 0u32;
    const MAX_ATTEMPTS: u32 = 20;

    loop {
        attempt += 1;
        bitrate_bps = (bitrate_bps as f64 * 0.9) as u32;
        let bitrate_kbps = (bitrate_bps / 1000).max(32);
        log_trace(
            trace_id,
            "compress_attempt",
            &format!("attempt={attempt} bitrate={bitrate_kbps}k"),
        );

        let edit_text = crate::i18n::tf(
            "separation.compressing_audio",
            &[
                ("attempt", &attempt.to_string()),
                ("max", &MAX_ATTEMPTS.to_string()),
            ],
        );
        let _ = api
            .edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(message_id)
                    .text(edit_text)
                    .build(),
            )
            .await;

        let out_path = tmp_dir.join(format!("compressed_{attempt}.mp3"));
        let out_path_str = out_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("invalid UTF-8 path"))?;
        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-i",
                audio_path_str,
                "-acodec",
                "libmp3lame",
                "-b:a",
                &format!("{bitrate_kbps}k"),
                out_path_str,
            ])
            .output()
            .await?;
        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            anyhow::bail!("ffmpeg compress failed: {stderr}");
        }

        let size = std::fs::metadata(&out_path)?.len();
        log_trace(
            trace_id,
            "compressed",
            &format!("attempt={attempt} size={size}"),
        );

        if size <= MAX_AUDIO_BYTES {
            return Ok(std::fs::read(&out_path)?);
        }

        if attempt >= MAX_ATTEMPTS || bitrate_kbps <= 32 {
            anyhow::bail!("audio still too large after max compression attempts");
        }
    }
}

async fn download_file(
    api: &Bot,
    file_id: &str,
    trace_id: u64,
) -> crate::error::Result<(Vec<u8>, crate::bot::TransferResult)> {
    let tmp_path = std::env::temp_dir().join(format!("sep_dl_{trace_id}.tmp"));
    let res = crate::bot::download_telegram_file(api, file_id, &tmp_path)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let bytes = std::fs::read(&tmp_path)?;
    let _ = std::fs::remove_file(&tmp_path);
    log_trace(
        trace_id,
        "http_done",
        &format!("bytes={} speed={}", bytes.len(), res.speed_human()),
    );
    Ok((bytes, res))
}

/// Formats seconds into mm:ss or hh:mm:ss.
fn format_clock(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

fn format_duration_fa(secs: u64) -> String {
    if secs < 3600 {
        let mins = secs / 60;
        tf("rank.duration_minutes", &[("mins", &mins.to_string())])
    } else {
        let hours = secs / 3600;
        let rem_mins = (secs % 3600) / 60;
        if rem_mins == 0 {
            tf("rank.duration_hours", &[("hours", &hours.to_string())])
        } else {
            tf(
                "rank.duration_hours_minutes",
                &[
                    ("hours", &hours.to_string()),
                    ("mins", &rem_mins.to_string()),
                ],
            )
        }
    }
}

async fn delete_message(api: &Bot, chat_id: i64, message_id: i32) -> crate::error::Result<()> {
    let params = DeleteMessageParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .build();
    api.delete_message(&params).await?;
    Ok(())
}
