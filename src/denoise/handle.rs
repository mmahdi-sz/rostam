use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};

use crate::common::job::JobRegistry;

static ACTIVE_DENOISE_JOBS: LazyLock<JobRegistry<i64>> = LazyLock::new(JobRegistry::new);

pub fn register_denoise_cancel(user_id: i64) -> Arc<AtomicBool> {
    ACTIVE_DENOISE_JOBS.register(user_id)
}

pub fn unregister_denoise_cancel(user_id: i64) {
    ACTIVE_DENOISE_JOBS.unregister(&user_id);
}

pub fn cancel_denoise_job(user_id: i64) -> bool {
    ACTIVE_DENOISE_JOBS.cancel(&user_id)
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
use crate::common::cpu_broker::CpuBrokerGuard;
use crate::common::dir::TempDirGuard;
use crate::common::ffmpeg::{convert_to_wav, probe_metadata};
use crate::common::keyboard::job_cancel_keyboard;
use crate::common::ticker::ProgressTicker;
use crate::database::postgresql::PostgresDatabase;
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
    job_cancel_keyboard(&t("denoise.cancel_button"), CB_DENOISE_CANCEL, "cancel")
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
    if CpuBrokerGuard::is_user_busy(user_id).await {
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

    let dir_guard = match TempDirGuard::create("denoise", trace_id) {
        Ok(g) => g,
        Err(e) => {
            log_trace(trace_id, "denoise_temp_dir_failed", &e.to_string());
            let _ = send_text_with_back(api, chat_id, &t("denoise.convert_failed")).await;
            return;
        }
    };
    let work_dir = dir_guard.path().to_path_buf();

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
    if let Err(e) = convert_to_wav(
        std::path::Path::new(input_str),
        std::path::Path::new(wav_str),
        48000,
        1,
    )
    .await
    {
        log_trace(trace_id, "denoise_convert_failed", &format!("err={e}"));
        crate::stats::record_event_user(user_id, "denoise", "", "fail", 0).await;
        crate::stats::record_error_global("denoise", &format!("convert failed: {e}")).await;
        let _ = send_text_with_back(api, chat_id, &t("denoise.convert_failed")).await;
        clean_up(&work_dir);
        return;
    }
    log_trace(trace_id, "denoise_converted", "");

    // Determine audio duration from WAV metadata
    let audio_duration = probe_metadata(std::path::Path::new(wav_str))
        .await
        .map(|m| m.duration_exact)
        .unwrap_or(0.0);
    let duration_secs = audio_duration.ceil() as u64;

    // Reserve quota upfront (check-then-deduct previously allowed dual race calls).
    // Shorter window — daily — reserved first; if weekly fails, daily is refunded.
    //
    // Reserve at least 1s. If WAV header read fails, duration becomes 0 and 0 reservation
    // would always pass, allowing free work on exhausted quota.
    let reserve_secs = duration_secs.max(1) as i64;
    let mut reserved = false;
    if let Some(db) = database.as_ref() {
        let (user_rank, daily_limit, weekly_limit) = {
            let client = match db.get().await {
                Ok(c) => c,
                Err(e) => {
                    log_trace(
                        trace_id,
                        "denoise_quota_checkout",
                        &format!("err={e} => fail"),
                    );
                    clean_up(&work_dir);
                    crate::rank::paywall::quota_db_error(api, chat_id, "denoise", &format!("{e}"))
                        .await;
                    return;
                }
            };
            let user_rank = rank::effective_rank(&client, user_id).await;
            let d_lim = user_rank.denoise_daily_secs();
            let w_lim = user_rank.denoise_weekly_secs();
            (user_rank, d_lim, w_lim)
        };

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
                let (d_used, w_used) = if let Ok(client) = db.get().await {
                    let d = get_usage(&client, user_id, QuotaKind::DenoiseDaily, 86400)
                        .await
                        .unwrap_or(0) as u64;
                    let w = get_usage(&client, user_id, QuotaKind::DenoiseWeekly, 7 * 86400)
                        .await
                        .unwrap_or(0) as u64;
                    (d, w)
                } else {
                    (0, 0)
                };
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

        let (daily_res, weekly_res) = {
            let client = match db.get().await {
                Ok(c) => c,
                Err(e) => db_fail!(e),
            };
            let d_res = reserve_usage(
                &client,
                user_id,
                QuotaKind::DenoiseDaily,
                reserve_secs,
                86400,
                daily_limit as i64,
            )
            .await;
            let w_res = if matches!(d_res, Ok(Some(_))) {
                let w = reserve_usage(
                    &client,
                    user_id,
                    QuotaKind::DenoiseWeekly,
                    reserve_secs,
                    7 * 86400,
                    weekly_limit as i64,
                )
                .await;
                if !matches!(w, Ok(Some(_))) {
                    if let Err(e) = refund_usage(
                        &client,
                        user_id,
                        QuotaKind::DenoiseDaily,
                        reserve_secs,
                        86400,
                    )
                    .await
                    {
                        log_trace(trace_id, "denoise_quota_refund_failed", &e.to_string());
                        crate::stats::record_error_global(
                            "denoise",
                            &format!("refund_failed: {e}"),
                        )
                        .await;
                    }
                }
                Some(w)
            } else {
                None
            };
            (d_res, w_res)
        };

        match daily_res {
            Ok(Some(used)) => log_trace(
                trace_id,
                "denoise_quota_reserved_daily",
                &format!("used={used} limit={daily_limit}"),
            ),
            Ok(None) => deny!("denoise_quota_daily"),
            Err(e) => db_fail!(e),
        }

        if let Some(w_res) = weekly_res {
            match w_res {
                Ok(Some(used)) => {
                    reserved = true;
                    log_trace(
                        trace_id,
                        "denoise_quota_reserved_weekly",
                        &format!("used={used} limit={weekly_limit}"),
                    );
                }
                Ok(None) => deny!("denoise_quota_weekly"),
                Err(e) => db_fail!(e),
            }
        }
    }

    // Refund both windows when job fails
    macro_rules! refund {
        ($why:expr) => {
            if reserved {
                if let Some(db) = database.as_ref() {
                    log_trace(trace_id, "denoise_quota_refund", &format!("why={}", $why));
                    if let Ok(client) = db.get().await {
                        for (kind, window) in [
                            (QuotaKind::DenoiseDaily, 86400),
                            (QuotaKind::DenoiseWeekly, 7 * 86400),
                        ] {
                            if let Err(e) =
                                refund_usage(&client, user_id, kind, reserve_secs, window).await
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
            }
        };
    }

    // 3. Denoise via DeepFilterNet — blocking (std::process::Command), run on thread pool.
    let est_total_secs = (audio_duration / 5.1).max(2.0);
    let progress_ticker = status_msg_id.map(|msg_id| {
        ProgressTicker::new(api, chat_id, msg_id)
            .interval(std::time::Duration::from_millis(1500))
            .with_cancel_flag(cancel_flag.clone())
            .with_keyboard(denoise_keyboard())
            .spawn(move |elapsed| {
                let elapsed_secs = elapsed.as_secs_f64();
                let percent = ((elapsed_secs / est_total_secs) * 100.0).min(99.0) as f32;
                let eta_secs = (est_total_secs - elapsed_secs).max(0.0);

                let bar = crate::youtube::download::progress::build_bar(percent);
                let elapsed_str = crate::youtube::download::progress::format_elapsed(elapsed);
                let eta_str = crate::youtube::download::progress::format_elapsed(
                    std::time::Duration::from_secs_f64(eta_secs),
                );

                Some(apply_premium_to_md(&tf(
                    "denoise.progress",
                    &[
                        ("bar", &bar),
                        ("percent", &format!("{percent:.0}")),
                        ("elapsed", &elapsed_str),
                        ("eta", &eta_str),
                    ],
                )))
            })
    });

    let mut cpu_guard = CpuBrokerGuard::acquire(user_id, trace_id, "denoise").await;

    let denoise_res = {
        let wav_in = wav_str.to_string();
        let wav_out = denoised_str.to_string();
        let guard_cores = cpu_guard.cores().to_vec();
        tokio::task::spawn_blocking(move || {
            if !guard_cores.is_empty() {
                crate::moebius::cpu::pin_current_thread(&guard_cores, trace_id);
            }
            deepfilter::denoise(&wav_in, &wav_out).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("denoise task panicked: {e}")))
    };

    cpu_guard.release().await;

    if let Some(ticker) = progress_ticker {
        ticker.stop();
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

    // 4. Convert back to original format
    let reconvert_res = if is_video {
        convert_from_wav_video(input_str, denoised_str, output_str).await
    } else {
        convert_from_wav(denoised_str, output_str, &orig_ext).await
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
async fn convert_from_wav(input: &str, output: &str, ext: &str) -> crate::error::Result<()> {
    let status = match ext {
        "ogg" => tokio::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "libopus", "-b:a", "32k", output])
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("ffmpeg failed: {e}"))?,
        "mp3" => tokio::process::Command::new("ffmpeg")
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
            .await
            .map_err(|e| anyhow::anyhow!("ffmpeg failed: {e}"))?,
        "m4a" => tokio::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "aac", "-b:a", "128k", output])
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("ffmpeg failed: {e}"))?,
        "flac" => tokio::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "flac", output])
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("ffmpeg failed: {e}"))?,
        "webm" => tokio::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "libopus", output])
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("ffmpeg failed: {e}"))?,
        // wav: just copy the denoised wav
        "wav" => {
            tokio::fs::copy(input, output).await?;
            return Ok(());
        }
        _ => {
            // fallback: copy wav as-is
            tokio::fs::copy(input, output).await?;
            return Ok(());
        }
    };
    if !status.success() {
        anyhow::bail!("ffmpeg reconversion failed");
    }
    Ok(())
}

async fn convert_from_wav_video(
    video_input: &str,
    wav_input: &str,
    output_video: &str,
) -> crate::error::Result<()> {
    let status = tokio::process::Command::new("ffmpeg")
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
        .await
        .map_err(|e| anyhow::anyhow!("ffmpeg video remux failed: {e}"))?;
    if !status.success() {
        anyhow::bail!("ffmpeg video remux failed");
    }
    Ok(())
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

    #[test]
    fn test_denoise_cancel_lifecycle() {
        let user_id = 999_123_456;
        let flag = register_denoise_cancel(user_id);
        assert!(!flag.load(Ordering::SeqCst));

        // Cancel job
        let cancelled = cancel_denoise_job(user_id);
        assert!(cancelled);
        assert!(flag.load(Ordering::SeqCst));

        // Second cancel should return false since it's already removed
        assert!(!cancel_denoise_job(user_id));

        // Unregister
        let flag2 = register_denoise_cancel(user_id);
        assert!(!flag2.load(Ordering::SeqCst));
        {
            let _guard = DenoiseUnregisterGuard(user_id);
        }
        // Guard drop unregisters it
        assert!(!cancel_denoise_job(user_id));
    }
}
