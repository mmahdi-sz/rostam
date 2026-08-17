use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static ACTIVE_DENOISE_JOBS: OnceLock<Mutex<HashMap<i64, Arc<AtomicBool>>>> = OnceLock::new();

fn active_denoise_jobs() -> &'static Mutex<HashMap<i64, Arc<AtomicBool>>> {
    ACTIVE_DENOISE_JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_denoise_cancel(user_id: i64) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    crate::sync_util::lock_or_recover(active_denoise_jobs()).insert(user_id, flag.clone());
    flag
}

pub fn unregister_denoise_cancel(user_id: i64) {
    crate::sync_util::lock_or_recover(active_denoise_jobs()).remove(&user_id);
}

pub fn cancel_denoise_job(user_id: i64) -> bool {
    if let Some(flag) = crate::sync_util::lock_or_recover(active_denoise_jobs()).remove(&user_id) {
        flag.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

pub struct DenoiseUnregisterGuard(pub i64);
impl Drop for DenoiseUnregisterGuard {
    fn drop(&mut self) {
        unregister_denoise_cancel(self.0);
    }
}

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{
        DeleteMessageParams, EditMessageTextParams, SendAudioParams, SendMessageParams,
        SendVideoParams, SendVoiceParams,
    },
    types::{InlineKeyboardMarkup, Message},
};

use crate::bot::{
    CB_DENOISE_CANCEL, ai_lab_back_keyboard, send_text, send_text_md_with_keyboard,
    send_text_with_back,
};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::panel::btn_icon_danger;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t, tf};
use crate::log::next_trace_id;
use crate::rank::{
    self,
    quota::{QuotaKind, get_usage, refund_usage, reserve_usage},
};
use crate::stt::deepfilter;

// ponytail: thin shim so existing log_trace() calls below keep working with correct domain.
fn log_trace(trace_id: u64, event: &str, details: &str) {
    crate::log::emit("denoise", trace_id, event, details);
}

/// Called from main.rs when `ai:denoise` is pressed.
/// Edits the AI Lab message to show the denoise prompt.
pub async fn enter_denoise(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.set(user_id, FlowState::AwaitingDenoiseAudio);
    log_actor_id!("denoise", trace_id, user_id, "clicked" => "ai:denoise");

    let text = apply_premium_to_md(&t("denoise.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(denoise_keyboard())
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => log_trace(
            trace_id,
            "denoise_prompt_shown",
            &format!("user_id={user_id} chat_id={chat_id}"),
        ),
        Err(e) => log_trace(trace_id, "denoise_prompt_failed", &e.to_string()),
    }
}

fn denoise_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("denoise.cancel_button"),
            CB_DENOISE_CANCEL,
            "cancel",
        )]])
        .build()
}

/// Handles denoise cancel callback — back to AI Lab.
pub async fn handle_denoise_cancel(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    flow_manager.clear(user_id);
    let cancelled = cancel_denoise_job(user_id);
    let r = crate::bot::edit_to_ai_lab(api, chat_id, message_id).await;
    log_trace(
        next_trace_id(),
        "denoise_cancel_done",
        &format!("user_id={user_id} cancelled={cancelled} ok={}", r.is_ok()),
    );
}

/// Processes an audio message when user is in AwaitingDenoiseAudio.
pub async fn handle_denoise_audio(
    api: &Bot,
    message: &Message,
    user_id: i64,
    database: &Option<PostgresDatabase>,
) {
    if crate::moebius::cpu::is_user_cpu_busy(user_id).await {
        let _ = send_text(api, message.chat.id, &t("active_job_running")).await;
        return;
    }

    // Flow state is cleared by the dispatcher before spawning this task.
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    let cancel_flag = register_denoise_cancel(user_id);
    let _cancel_guard = DenoiseUnregisterGuard(user_id);
    log_actor_id!("denoise", trace_id, user_id, "clicked" => "audio/voice");

    let file_id = message
        .voice
        .as_ref()
        .map(|v| &v.file_id)
        .or_else(|| message.audio.as_ref().map(|a| &a.file_id))
        .or_else(|| message.video.as_ref().map(|v| &v.file_id))
        .or_else(|| message.document.as_ref().map(|d| &d.file_id));

    let Some(file_id) = file_id else {
        let _ = send_text(api, chat_id, &t("stt.unsupported_format")).await;
        return;
    };

    let is_voice = message.voice.is_some();
    let is_audio = message.audio.is_some();
    let is_video = message.video.is_some();
    let is_doc = message.document.is_some();
    let orig_ext = detect_format(message);

    // Extract original filename for output naming
    let orig_stem = message
        .audio
        .as_ref()
        .and_then(|a| a.file_name.as_deref())
        .or_else(|| message.video.as_ref().and_then(|v| v.file_name.as_deref()))
        .or_else(|| {
            message
                .document
                .as_ref()
                .and_then(|d| d.file_name.as_deref())
        })
        .and_then(|name| {
            let dot = name.rfind('.')?;
            Some(&name[..dot])
        })
        .unwrap_or(if is_video { "video" } else { "voice" });
    let clean_filename = format!("{orig_stem}_clean.{orig_ext}");

    log_trace(
        trace_id,
        "denoise_audio_received",
        &format!(
            "user_id={user_id} chat_id={chat_id} voice={is_voice} audio={is_audio} video={is_video} doc={is_doc} ext={orig_ext} stem={orig_stem} clean={clean_filename}"
        ),
    );

    let status_msg_id = match api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(t("denoise.preparing"))
                .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(
                    denoise_keyboard(),
                ))
                .build(),
        )
        .await
    {
        Ok(m) => Some(m.result.message_id),
        Err(_) => None,
    };

    let work_dir = std::env::temp_dir().join(format!("denoise_{trace_id}"));
    std::fs::create_dir_all(&work_dir).ok();

    let input_path = work_dir.join(format!("input.{orig_ext}"));
    let wav_path = work_dir.join("denoise_input.wav");
    let denoised_path = work_dir.join("denoised.wav");
    let output_path = work_dir.join(&clean_filename);

    let (Some(input_str), Some(wav_str), Some(denoised_str), Some(output_str)) = (
        input_path.to_str(),
        wav_path.to_str(),
        denoised_path.to_str(),
        output_path.to_str(),
    ) else {
        log_trace(trace_id, "denoise_invalid_path", "invalid UTF-8 path");
        clean_up(&work_dir);
        return;
    };

    let stats_job_id = crate::stats::record_download_start(user_id, "denoise").await;

    // 1. Download
    let dl_result = match download_file(api, file_id, input_str).await {
        Ok(res) => res,
        Err(e) => {
            log_trace(trace_id, "denoise_download_failed", &format!("err={e}"));
            crate::stats::record_event_user(user_id, "denoise", "", "fail", 0).await;
            crate::stats::record_error_global("denoise", &format!("download failed: {e}")).await;
            let _ = send_text_with_back(api, chat_id, &t("denoise.download_failed")).await;
            clean_up(&work_dir);
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

    let file_size = std::fs::metadata(input_str).map(|m| m.len()).unwrap_or(0);
    log_trace(
        trace_id,
        "denoise_downloaded",
        &format!("size={file_size} speed={}", dl_result.speed_human()),
    );

    // 2. Convert to 48kHz mono 16-bit PCM WAV (DeepFilterNet optimal sample rate)
    // Blocking (std::process::Command) — run on the blocking thread pool.
    let convert_res = {
        let inp = input_str.to_string();
        let outp = wav_str.to_string();
        tokio::task::spawn_blocking(move || {
            convert_to_wav(&inp, &outp, 48000).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("convert task panicked: {e}")))
    };
    if let Err(e) = convert_res {
        log_trace(trace_id, "denoise_convert_failed", &format!("err={e}"));
        crate::stats::record_event_user(user_id, "denoise", "", "fail", 0).await;
        crate::stats::record_error_global("denoise", &format!("convert failed: {e}")).await;
        let _ = send_text_with_back(api, chat_id, &t("denoise.convert_failed")).await;
        clean_up(&work_dir);
        return;
    }
    log_trace(trace_id, "denoise_converted", "");

    // Determine audio duration from WAV header
    let audio_duration = wav_duration(wav_str).unwrap_or(0.0);
    let duration_secs = audio_duration.ceil() as u64;

    // Reserve quota upfront (check-then-deduct previously allowed dual race calls).
    // Shorter window — daily — reserved first; if weekly fails, daily is refunded.
    //
    // Reserve at least 1s. If WAV header read fails, duration becomes 0 and 0 reservation
    // would always pass, allowing free work on exhausted quota.
    let reserve_secs = duration_secs.max(1) as i64;
    let mut reserved = false;
    if let Some(db) = database.as_ref() {
        let user_rank = rank::effective_rank(db.client(), user_id).await;
        let daily_limit = user_rank.denoise_daily_secs();
        let weekly_limit = user_rank.denoise_weekly_secs();

        // fail closed — notify user on DB error
        macro_rules! db_fail {
            ($e:expr) => {{
                log_trace(
                    trace_id,
                    "denoise_quota_reserve",
                    &format!("err={} => fail", $e),
                );
                clean_up(&work_dir);
                crate::rank::paywall::quota_db_error(api, chat_id, "denoise", &format!("{}", $e))
                    .await;
                return;
            }};
        }

        // Deny message has 3 reasons (daily quota / weekly quota / file exceeds remaining).
        // Reservation only signals failure, so read usage here only on deny path.
        macro_rules! deny {
            ($event:expr) => {{
                let d_used = get_usage(db.client(), user_id, QuotaKind::DenoiseDaily, 86400)
                    .await
                    .unwrap_or(0) as u64;
                let w_used = get_usage(db.client(), user_id, QuotaKind::DenoiseWeekly, 7 * 86400)
                    .await
                    .unwrap_or(0) as u64;
                let d_rem = daily_limit.saturating_sub(d_used);
                let w_rem = weekly_limit.saturating_sub(w_used);
                let (rank_key, user_key, ph, val) = if d_rem == 0 {
                    (
                        "rank.denoise_daily_limit",
                        "denoise.quota_daily_exceeded",
                        "limit",
                        format_duration_fa(daily_limit),
                    )
                } else if w_rem == 0 {
                    (
                        "rank.denoise_weekly_limit",
                        "denoise.quota_weekly_exceeded",
                        "limit",
                        format_duration_fa(weekly_limit),
                    )
                } else {
                    (
                        "rank.denoise_remaining",
                        "denoise.quota_file_too_long",
                        "remaining",
                        format_duration_fa(d_rem.min(w_rem)),
                    )
                };
                log_trace(
                    trace_id,
                    $event,
                    &format!(
                        "user_id={user_id} daily_used={d_used} weekly_used={w_used} duration={duration_secs} => blocked"
                    ),
                );
                clean_up(&work_dir);
                if let Some(min_rank) = user_rank.denoise_next_rank() {
                    let label = tf(rank_key, &[(ph, &val)]);
                    crate::rank::paywall::block_limit(api, chat_id, &label, min_rank).await;
                } else {
                    let _ = send_text_with_back(api, chat_id, &tf(user_key, &[(ph, &val)])).await;
                }
                return;
            }};
        }

        match reserve_usage(
            db.client(),
            user_id,
            QuotaKind::DenoiseDaily,
            reserve_secs,
            86400,
            daily_limit as i64,
        )
        .await
        {
            Ok(Some(used)) => log_trace(
                trace_id,
                "denoise_quota_reserved_daily",
                &format!("used={used} limit={daily_limit}"),
            ),
            Ok(None) => deny!("denoise_quota_daily"),
            Err(e) => db_fail!(e),
        }

        match reserve_usage(
            db.client(),
            user_id,
            QuotaKind::DenoiseWeekly,
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
                    "denoise_quota_reserved_weekly",
                    &format!("used={used} limit={weekly_limit}"),
                );
            }
            Ok(None) => {
                let _ = refund_usage(
                    db.client(),
                    user_id,
                    QuotaKind::DenoiseDaily,
                    reserve_secs,
                    86400,
                )
                .await;
                deny!("denoise_quota_weekly");
            }
            Err(e) => {
                let _ = refund_usage(
                    db.client(),
                    user_id,
                    QuotaKind::DenoiseDaily,
                    reserve_secs,
                    86400,
                )
                .await;
                db_fail!(e);
            }
        }
    }

    // Refund both windows when job fails
    macro_rules! refund {
        ($why:expr) => {
            if reserved {
                if let Some(db) = database.as_ref() {
                    log_trace(trace_id, "denoise_quota_refund", &format!("why={}", $why));
                    for (kind, window) in [
                        (QuotaKind::DenoiseDaily, 86400),
                        (QuotaKind::DenoiseWeekly, 7 * 86400),
                    ] {
                        if let Err(e) =
                            refund_usage(db.client(), user_id, kind, reserve_secs, window).await
                        {
                            log_trace(
                                trace_id,
                                "denoise_quota_refund",
                                &format!("err={e} => fail"),
                            );
                            crate::stats::record_error_global("denoise", "quota_refund_failed")
                                .await;
                        }
                    }
                }
            }
        };
    }

    // 3. Denoise via DeepFilterNet — blocking (std::process::Command), run on thread pool.
    let est_total_secs = (audio_duration / 5.1).max(2.0);
    let cancel_flag_ticker = cancel_flag.clone();
    let progress_ticker = if let Some(msg_id) = status_msg_id {
        let api_clone = api.clone();
        let start_inst = std::time::Instant::now();
        Some(crate::app::spawn_user_task(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(1500));
            loop {
                interval.tick().await;
                if cancel_flag_ticker.load(Ordering::SeqCst) {
                    break;
                }
                let elapsed_secs = start_inst.elapsed().as_secs_f64();
                let percent = ((elapsed_secs / est_total_secs) * 100.0).min(99.0) as f32;
                let eta_secs = (est_total_secs - elapsed_secs).max(0.0);

                let bar = crate::youtube::download::progress::build_bar(percent);
                let elapsed_str =
                    crate::youtube::download::progress::format_elapsed(start_inst.elapsed());
                let eta_str = crate::youtube::download::progress::format_elapsed(
                    std::time::Duration::from_secs_f64(eta_secs),
                );

                let text = apply_premium_to_md(&tf(
                    "denoise.progress",
                    &[
                        ("bar", &bar),
                        ("percent", &format!("{percent:.0}")),
                        ("elapsed", &elapsed_str),
                        ("eta", &eta_str),
                    ],
                ));

                if cancel_flag_ticker.load(Ordering::SeqCst) {
                    break;
                }

                let params = EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(msg_id)
                    .text(&text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(denoise_keyboard())
                    .build();
                if let Err(e) = api_clone.edit_message_text(&params).await {
                    let desc = e.to_string();
                    if !desc.contains("message is not modified") {
                        log_trace(trace_id, "denoise_progress_edit_failed", &desc);
                    }
                }
            }
        }))
    } else {
        None
    };

    let cores = crate::moebius::cpu::acquire_cpu(user_id, trace_id).await;
    let cores_clone = cores.clone();

    let denoise_res = {
        let wav_in = wav_str.to_string();
        let wav_out = denoised_str.to_string();
        tokio::task::spawn_blocking(move || {
            crate::moebius::cpu::pin_current_thread(&cores_clone, trace_id);
            deepfilter::denoise(&wav_in, &wav_out).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("denoise task panicked: {e}")))
    };

    crate::moebius::cpu::release_cpu(cores, trace_id).await;

    if let Some(ticker) = progress_ticker {
        ticker.abort();
    }

    if cancel_flag.load(Ordering::SeqCst) {
        log_trace(
            trace_id,
            "denoise_cancelled_at_completion",
            &format!("user_id={user_id}"),
        );
        clean_up(&work_dir);
        refund!("cancelled_at_completion");
        return;
    }
    let processing_secs = match denoise_res {
        Ok(s) => {
            log_trace(trace_id, "denoise_done", &format!("elapsed={s:.1}s"));
            s
        }
        Err(e) => {
            log_trace(trace_id, "denoise_failed", &format!("err={e}"));
            crate::stats::record_event_user(user_id, "denoise", "", "fail", duration_secs as i64)
                .await;
            crate::stats::record_error_global("denoise", &format!("denoise failed: {e}")).await;
            let _ = send_text_with_back(api, chat_id, &t("denoise.denoise_failed")).await;
            clean_up(&work_dir);
            refund!("denoise_failed");
            return;
        }
    };

    // 4. Convert back to original format — blocking, run on thread pool.
    let reconvert_res = {
        let inp = denoised_str.to_string();
        let outp = output_str.to_string();
        let orig_inp = input_str.to_string();
        let ext = orig_ext.clone();
        tokio::task::spawn_blocking(move || {
            if is_video {
                convert_from_wav_video(&orig_inp, &inp, &outp).map_err(|e| e.to_string())
            } else {
                convert_from_wav(&inp, &outp, &ext).map_err(|e| e.to_string())
            }
        })
        .await
        .unwrap_or_else(|e| Err(format!("reconvert task panicked: {e}")))
    };
    if let Err(e) = reconvert_res {
        log_trace(trace_id, "denoise_reconvert_failed", &format!("err={e}"));
        crate::stats::record_event_user(user_id, "denoise", "", "fail", duration_secs as i64).await;
        crate::stats::record_error_global("denoise", &format!("reconvert failed: {e}")).await;
        let _ = send_text_with_back(api, chat_id, &t("denoise.convert_failed")).await;
        clean_up(&work_dir);
        refund!("reconvert_failed");
        return;
    }
    log_trace(
        trace_id,
        "denoise_reconverted",
        &format!("ext={orig_ext} video={is_video}"),
    );

    if cancel_flag.load(Ordering::SeqCst) {
        log_trace(
            trace_id,
            "denoise_cancelled_before_send",
            &format!("user_id={user_id}"),
        );
        clean_up(&work_dir);
        refund!("cancelled_before_send");
        return;
    }

    // 5. Send denoised file
    let efficiency = if processing_secs > 0.0 {
        audio_duration / processing_secs
    } else {
        0.0
    };

    let caption = apply_premium_to_md(&t("denoise.result_caption"));

    let out_bytes = std::fs::metadata(output_str).map(|m| m.len()).unwrap_or(0);
    let up_start = std::time::Instant::now();

    use crate::bot::send_file_with_upload_ticker;
    let smid = status_msg_id.unwrap_or(0);
    let upload_success = if is_voice {
        let params = SendVoiceParams::builder()
            .chat_id(chat_id)
            .voice(PathBuf::from(output_str))
            .caption(&caption)
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        let r = send_file_with_upload_ticker::<_, frankenstein::types::Message>(
            api,
            "sendVoice",
            &params,
            std::path::Path::new(output_str),
            chat_id,
            smid,
            "transfer.stage.sending_audio",
            None,
        )
        .await;
        log_trace(trace_id, "denoise_voice_sent", &format!("ok={}", r.is_ok()));
        r.is_ok()
    } else if is_video {
        let params = SendVideoParams::builder()
            .chat_id(chat_id)
            .video(PathBuf::from(output_str))
            .caption(&caption)
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        let r = send_file_with_upload_ticker::<_, frankenstein::types::Message>(
            api,
            "sendVideo",
            &params,
            std::path::Path::new(output_str),
            chat_id,
            smid,
            "transfer.stage.sending_video",
            None,
        )
        .await;
        log_trace(trace_id, "denoise_video_sent", &format!("ok={}", r.is_ok()));
        r.is_ok()
    } else {
        let params = SendAudioParams::builder()
            .chat_id(chat_id)
            .audio(PathBuf::from(output_str))
            .caption(&caption)
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        let r = send_file_with_upload_ticker::<_, frankenstein::types::Message>(
            api,
            "sendAudio",
            &params,
            std::path::Path::new(output_str),
            chat_id,
            smid,
            "transfer.stage.sending_audio",
            None,
        )
        .await;
        log_trace(trace_id, "denoise_audio_sent", &format!("ok={}", r.is_ok()));
        r.is_ok()
    };

    if upload_success {
        let up_elapsed = up_start.elapsed();
        let up_speed = if up_elapsed.as_secs_f64() > 0.0 {
            out_bytes as f64 / up_elapsed.as_secs_f64()
        } else {
            0.0
        };
        if let Some(jid) = stats_job_id {
            crate::stats::record_upload_done(
                jid,
                user_id,
                out_bytes as i64,
                Some(up_speed as i64),
                Some(1),
            )
            .await;
        }
    }

    if let Some(msg_id) = status_msg_id {
        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(msg_id)
                    .build(),
            )
            .await;
    }

    // 6. Quota deducted during reservation; no secondary charge here

    // 7. Send report
    let duration_str = escape_md(&format!("{audio_duration:.1}"));
    let processing_str = escape_md(&format!("{processing_secs:.1}"));
    let ratio_str = escape_md(&format!("{efficiency:.1}"));
    let report = apply_premium_to_md(&tf(
        "denoise.report",
        &[
            ("duration", &duration_str),
            ("processing", &processing_str),
            ("ratio", &ratio_str),
        ],
    ));
    let kb = ai_lab_back_keyboard();
    let _ = send_text_md_with_keyboard(api, chat_id, &report, kb).await;
    log_trace(
        trace_id,
        "denoise_report_sent",
        &format!("duration={audio_duration:.1}s processing={processing_secs:.1}s"),
    );
    crate::stats::record_event_user(user_id, "denoise", "", "ok", duration_secs as i64).await;

    clean_up(&work_dir);
}

fn detect_format(message: &Message) -> String {
    if message.voice.is_some() {
        return "ogg".to_string();
    }
    if let Some(video) = &message.video {
        if let Some(name) = &video.file_name {
            if let Some(ext) = name.rsplit('.').next() {
                return ext.to_lowercase();
            }
        }
        if let Some(mime) = &video.mime_type {
            return mime_to_ext(mime);
        }
        return "mp4".to_string();
    }
    if let Some(audio) = &message.audio {
        if let Some(mime) = &audio.mime_type {
            return mime_to_ext(mime);
        }
        if let Some(name) = &audio.file_name {
            if let Some(ext) = name.rsplit('.').next() {
                return ext.to_lowercase();
            }
        }
    }
    if let Some(doc) = &message.document {
        if let Some(name) = &doc.file_name {
            if let Some(ext) = name.rsplit('.').next() {
                return ext.to_lowercase();
            }
        }
        if let Some(mime) = &doc.mime_type {
            return mime_to_ext(mime);
        }
    }
    "wav".to_string()
}

fn mime_to_ext(mime: &str) -> String {
    match mime {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/aac" | "video/mp4" => "mp4",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/wav" | "audio/wave" => "wav",
        "audio/flac" => "flac",
        "audio/webm" | "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        _ => "wav",
    }
    .to_string()
}

fn convert_to_wav(input: &str, output: &str, sample_rate: u32) -> crate::error::Result<()> {
    let output = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            input,
            "-ar",
            &sample_rate.to_string(),
            "-ac",
            "1",
            "-sample_fmt",
            "s16",
            "-f",
            "wav",
            output,
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("ffmpeg spawn failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffmpeg conversion failed: {stderr}");
    }
    Ok(())
}

fn convert_from_wav(input: &str, output: &str, ext: &str) -> crate::error::Result<()> {
    let status = match ext {
        "ogg" => std::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "libopus", "-b:a", "32k", output])
            .status()
            .map_err(|e| anyhow::anyhow!("ffmpeg failed: {e}"))?,
        "mp3" => std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-i",
                input,
                "-c:a",
                "libmp3lame",
                "-b:a",
                "128k",
                output,
            ])
            .status()
            .map_err(|e| anyhow::anyhow!("ffmpeg failed: {e}"))?,
        "m4a" => std::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "aac", "-b:a", "128k", output])
            .status()
            .map_err(|e| anyhow::anyhow!("ffmpeg failed: {e}"))?,
        "flac" => std::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "flac", output])
            .status()
            .map_err(|e| anyhow::anyhow!("ffmpeg failed: {e}"))?,
        "webm" => std::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "libopus", output])
            .status()
            .map_err(|e| anyhow::anyhow!("ffmpeg failed: {e}"))?,
        // wav: just copy the denoised wav
        "wav" => {
            std::fs::copy(input, output)?;
            return Ok(());
        }
        _ => {
            // fallback: copy wav as-is
            std::fs::copy(input, output)?;
            return Ok(());
        }
    };
    if !status.success() {
        anyhow::bail!("ffmpeg reconversion failed");
    }
    Ok(())
}

fn convert_from_wav_video(
    video_input: &str,
    wav_input: &str,
    output_video: &str,
) -> crate::error::Result<()> {
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            video_input,
            "-i",
            wav_input,
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-shortest",
            output_video,
        ])
        .status()
        .map_err(|e| anyhow::anyhow!("ffmpeg video remux failed: {e}"))?;
    if !status.success() {
        anyhow::bail!("ffmpeg video remux failed");
    }
    Ok(())
}

fn wav_duration(path: &str) -> crate::error::Result<f64> {
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
            path,
        ])
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(s.parse()?)
}

use crate::bot::download_telegram_file as download_file;

/// Escape MarkdownV2 special characters in dynamic text.
/// Does NOT touch `*` since those may be formatting markers in the i18n template.
fn escape_md(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '_' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|' | '{'
            | '}' | '.' | '!' => {
                format!("\\{c}")
            }
            other => other.to_string(),
        })
        .collect()
}

fn clean_up(dir: &std::path::Path) {
    std::fs::remove_dir_all(dir).ok();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_denoise_keyboard() {
        let kbd = denoise_keyboard();
        assert!(!kbd.inline_keyboard.is_empty());
    }

    #[test]
    fn test_escape_md() {
        assert_eq!(escape_md("hello_world"), "hello\\_world");
    }
}
