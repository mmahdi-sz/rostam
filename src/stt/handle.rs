use std::time::Instant;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::EditMessageTextParams,
};

use crate::bot::{edit_to_start_menu, send_text, send_long_text, send_text_with_back};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{t, tf};
use crate::rank::{self, quota::{QuotaKind, get_usage, add_usage}};
use crate::stt::config::*;
use crate::stt::deepfilter;
use crate::stt::types::{SttConfig, SttLang, SttModelSize};
use crate::stt::vosk;
use crate::log::next_trace_id;

use std::sync::{Arc, Mutex, LazyLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::HashMap;

// Global map tracking active STT jobs per user_id: user_id -> cancel_flag
static ACTIVE_STT_JOBS: LazyLock<Mutex<HashMap<i64, Arc<AtomicBool>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

// ponytail: thin shim — existing log_trace() calls below keep working with correct domain.
fn log_trace(trace_id: u64, event: &str, details: &str) {
    crate::log::emit("stt", trace_id, event, details);
}

// action قابل‌گرپ برای آمار: big/fast + fa/en + پسوند denoise.
fn stt_action(config: &SttConfig) -> String {
    let model = match config.model_size {
        SttModelSize::Large => "big",
        SttModelSize::Small => "fast",
    };
    let lang = match config.lang {
        SttLang::Fa => "fa",
        SttLang::En => "en",
    };
    if config.denoise { format!("{model}_{lang}_dn") } else { format!("{model}_{lang}") }
}

/// Called from main.rs when `ai:stt` is pressed.
/// Edits the AI Lab submenu message to show the STT config menu.
pub async fn enter_stt_config(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    log_actor_id!("stt", trace_id, user_id, "clicked" => "ai:stt");

    let denoise_default = if let Some(db) = database.as_ref() {
        let user_rank = rank::effective_rank(db.client(), user_id).await;
        user_rank.stt_denoise_default()
    } else {
        false
    };

    let config = SttConfig {
        lang: SttLang::Fa,
        model_size: SttModelSize::Large,
        denoise: denoise_default,
    };
    flow_manager.set(user_id, FlowState::AwaitingSttConfig { config: config.clone() });

    let text = t("stt.config_title");
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .reply_markup(config_keyboard(denoise_default))
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => log_trace(trace_id, "stt_config_shown", &format!("user_id={user_id} denoise_default={denoise_default}")),
        Err(e) => log_trace(trace_id, "stt_config_failed", &e.to_string()),
    }
}

/// Handles all `stt:*` callbacks.
pub async fn handle_stt_callback(
    api: &Bot,
    data: &str,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) -> bool {
    let trace_id = next_trace_id();
    log_actor_id!("stt", trace_id, user_id, "clicked" => data);

    match data {
        CB_STT_FA_BIG | CB_STT_FA_SMALL | CB_STT_EN_BIG | CB_STT_EN_SMALL => {
            let (lang, size) = match data {
                CB_STT_FA_BIG => (SttLang::Fa, SttModelSize::Large),
                CB_STT_FA_SMALL => (SttLang::Fa, SttModelSize::Small),
                CB_STT_EN_BIG => (SttLang::En, SttModelSize::Large),
                CB_STT_EN_SMALL => (SttLang::En, SttModelSize::Small),
                _ => unreachable!(),
            };

            // paywall — مدل دقیق فقط سهراب به بالا
            if size == SttModelSize::Large {
                if let Some(db) = database.as_ref() {
                    let user_rank = rank::effective_rank(db.client(), user_id).await;
                    if !user_rank.can_stt_accurate() {
                        log_trace(trace_id, "stt_accurate_paywall", &format!("user_id={user_id} rank={}", user_rank.as_str()));
                        crate::rank::paywall::block_feature(
                            api, chat_id,
                            &t("stt.accurate_feature_name"),
                            rank::types::Rank::Sohrab,
                        ).await;
                        return true;
                    }
                }
            }

            let state = flow_manager.get(user_id);
            let denoise = match &state {
                FlowState::AwaitingSttConfig { config } | FlowState::AwaitingSttAudio { config } => config.denoise,
                _ => true,
            };

            let config = SttConfig { lang, model_size: size, denoise };
            flow_manager.set(user_id, FlowState::AwaitingSttAudio { config: config.clone() });

            log_trace(trace_id, "stt_lang_chosen", &format!("user_id={user_id} lang={lang:?} size={size:?} denoise={denoise}"));

            let text = tf("stt.ready_title", &[("model", config.label_key())]);
            let params = EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(message_id)
                .text(&text)
                .reply_markup(ready_keyboard())
                .build();
            let _ = api.edit_message_text(&params).await;

            true
        }
        CB_STT_TOGGLE_DENOISE => {
            let state = flow_manager.get(user_id);
            let mut config = match &state {
                FlowState::AwaitingSttConfig { config } => config.clone(),
                FlowState::AwaitingSttAudio { config } => config.clone(),
                _ => return false,
            };

            // paywall — فعال کردن denoise فقط سهراب به بالا
            if !config.denoise {
                if let Some(db) = database.as_ref() {
                    let user_rank = rank::effective_rank(db.client(), user_id).await;
                    if !user_rank.can_stt_denoise() {
                        log_trace(trace_id, "stt_denoise_paywall", &format!("user_id={user_id} rank={}", user_rank.as_str()));
                        crate::rank::paywall::block_feature(
                            api, chat_id,
                            &crate::i18n::t("stt.denoise_feature_name"),
                            rank::types::Rank::Sohrab,
                        ).await;
                        return true;
                    }
                }
            }

            config.denoise = !config.denoise;

            let new_state = match &state {
                FlowState::AwaitingSttConfig { .. } => FlowState::AwaitingSttConfig { config: config.clone() },
                FlowState::AwaitingSttAudio { .. } => FlowState::AwaitingSttAudio { config: config.clone() },
                _ => return false,
            };
            flow_manager.set(user_id, new_state);

            log_trace(trace_id, "stt_toggle_denoise", &format!("denoise={}", config.denoise));

            let text = t("stt.config_title");
            let params = EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(message_id)
                .text(&text)
                .reply_markup(config_keyboard(config.denoise))
                .build();
            let _ = api.edit_message_text(&params).await;

            true
        }
        CB_STT_BACK => {
            log_trace(trace_id, "stt_back_to_ai_lab", &format!("user_id={user_id}"));
            flow_manager.clear(user_id);
            // Edit to AI Lab submenu — using bot::edit_to_ai_lab
            let r = crate::bot::edit_to_ai_lab(api, chat_id, message_id).await;
            log_trace(trace_id, "stt_back_done", &format!("ok={}", r.is_ok()));
            true
        }
        CB_STT_CANCEL => {
            log_trace(trace_id, "stt_cancel", &format!("user_id={user_id}"));
            flow_manager.clear(user_id);
            let r = crate::bot::edit_to_ai_lab(api, chat_id, message_id).await;
            log_trace(trace_id, "stt_cancel_done", &format!("ok={}", r.is_ok()));
            true
        }
        CB_STT_JOB_CANCEL => {
            log_trace(trace_id, "stt_job_cancel", &format!("user_id={user_id}"));
            if let Ok(mut jobs) = ACTIVE_STT_JOBS.lock() {
                if let Some(cancel_flag) = jobs.remove(&user_id) {
                    cancel_flag.store(true, Ordering::Relaxed);
                }
            }
            flow_manager.clear(user_id);
            let _ = api.delete_message(
                &frankenstein::methods::DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(message_id)
                    .build()
            ).await;
            let _ = send_text_with_back(api, chat_id, &t("stt.job_cancelled")).await;
            true
        }
        CB_STT_MAIN_MENU => {
            log_trace(trace_id, "stt_main_menu", &format!("user_id={user_id}"));
            flow_manager.clear(user_id);
            let r = edit_to_start_menu(api, chat_id, message_id).await;
            log_trace(trace_id, "stt_main_menu_done", &format!("ok={}", r.is_ok()));
            true
        }
        _ => false,
    }
}

/// Converts audio to 16kHz mono 16-bit PCM WAV using ffmpeg.
fn convert_to_wav(input: &str, output: &str) -> Result<(), Box<dyn std::error::Error>> {
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-y", "-i", input,
            "-ar", "16000", "-ac", "1", "-sample_fmt", "s16",
            "-f", "wav", output,
        ])
        .status()
        .map_err(|e| format!("ffmpeg failed: {e}"))?;

    if !status.success() {
        return Err("ffmpeg conversion failed".into());
    }
    Ok(())
}

fn remove_active_stt_job(user_id: i64) {
    if let Ok(mut jobs) = ACTIVE_STT_JOBS.lock() {
        jobs.remove(&user_id);
    }
}

/// Downloads a Telegram file by file_id to a local path.
use crate::bot::download_telegram_file as download_file;

/// Processes an audio message (voice or audio file) when the user is in AwaitingSttAudio.
/// Takes chat_id and file_id directly (already extracted in dispatch)
/// so this function can be spawned as a tokio task without cloning Message.
pub async fn handle_stt_audio(
    api: &Bot,
    chat_id: i64,
    file_id: &str,
    user_id: i64,
    config: &SttConfig,
    database: Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    log_actor_id!("stt", trace_id, user_id, "clicked" => "audio/voice");
    log_trace(trace_id, "stt_audio_received", &format!("user_id={user_id} chat_id={chat_id}"));

    // Register cancel flag for this user
    let cancel_flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut jobs) = ACTIVE_STT_JOBS.lock() {
        jobs.insert(user_id, cancel_flag.clone());
    }

    // ── Stage 1: Send initial status message & capture message_id ──
    let status_msg_id = match api.send_message(
        &frankenstein::methods::SendMessageParams::builder()
            .chat_id(chat_id)
            .text(&t("stt.stage_downloading"))
            .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(cancel_job_keyboard()))
            .build(),
    ).await {
        Ok(resp) => Some(resp.result.message_id),
        Err(_) => None,
    };

    let work_dir = std::env::temp_dir().join(format!("stt_{trace_id}"));
    std::fs::create_dir_all(&work_dir).ok();

    let input_path = work_dir.join("input");
    let wav_path = work_dir.join("converted.wav");
    let denoised_path = work_dir.join("denoised.wav");

    let (Some(input_str), Some(wav_str), Some(denoised_str)) =
        (input_path.to_str(), wav_path.to_str(), denoised_path.to_str()) else {
        log_trace(trace_id, "stt_invalid_path", "invalid UTF-8 path");
        remove_active_stt_job(user_id);
        clean_up(&work_dir);
        return;
    };

    let overall_start = Instant::now();

    // ── Stage 1: Download ──
    if let Err(e) = download_file(api, file_id, input_str).await.map_err(|e| e.to_string()) {
        remove_active_stt_job(user_id);
        log_trace(trace_id, "stt_download_failed", &format!("err={e}"));
        crate::stats::record_event_user(user_id, "stt", &stt_action(config), "fail", 0).await;
        crate::stats::record_error_global("stt", &format!("download failed: {e}")).await;
        let _ = send_text_with_back(api, chat_id, &t("stt.download_failed")).await;
        delete_status(api, chat_id, status_msg_id).await;
        clean_up(&work_dir);
        return;
    }
    log_trace(trace_id, "stt_downloaded", "");

    if cancel_flag.load(Ordering::Relaxed) {
        log_trace(trace_id, "stt_cancelled_after_download", "");
        remove_active_stt_job(user_id);
        clean_up(&work_dir);
        return;
    }

    // ── Stage 2: Convert to WAV ──
    edit_status(api, chat_id, status_msg_id, &t("stt.stage_converting")).await;

    let input_str_owned = input_str.to_string();
    let wav_str_owned = wav_str.to_string();
    if let Err(e) = tokio::task::spawn_blocking(move || {
        convert_to_wav(&input_str_owned, &wav_str_owned).map_err(|e| e.to_string())
    }).await.unwrap_or_else(|e| Err(e.to_string())) {
        remove_active_stt_job(user_id);
        log_trace(trace_id, "stt_convert_failed", &format!("err={e}"));
        crate::stats::record_event_user(user_id, "stt", &stt_action(config), "fail", 0).await;
        crate::stats::record_error_global("stt", &format!("convert failed: {e}")).await;
        let _ = send_text_with_back(api, chat_id, &t("stt.convert_failed")).await;
        delete_status(api, chat_id, status_msg_id).await;
        clean_up(&work_dir);
        return;
    }
    log_trace(trace_id, "stt_converted", "");

    if cancel_flag.load(Ordering::Relaxed) {
        log_trace(trace_id, "stt_cancelled_after_convert", "");
        remove_active_stt_job(user_id);
        clean_up(&work_dir);
        return;
    }

    let audio_duration = wav_duration(wav_str).unwrap_or(0.0);
    let duration_secs = audio_duration.ceil() as u64;

    // 3. Quota check — بعد از دونستن طول فایل
    let (daily_kind, weekly_kind) = match config.model_size {
        SttModelSize::Large => (QuotaKind::SttAccurateDaily, QuotaKind::SttAccurateWeekly),
        SttModelSize::Small => (QuotaKind::SttFastDaily, QuotaKind::SttFastWeekly),
    };
    if let Some(db) = database.as_ref() {
        let user_rank = rank::effective_rank(db.client(), user_id).await;
        let (daily_limit_opt, weekly_limit_opt) = match config.model_size {
            SttModelSize::Large => (user_rank.stt_accurate_daily_secs(), user_rank.stt_accurate_weekly_secs()),
            SttModelSize::Small => (user_rank.stt_fast_daily_secs(), user_rank.stt_fast_weekly_secs()),
        };
        let (daily_key, weekly_key, daily_limit_key, weekly_limit_key, file_key) = match config.model_size {
            SttModelSize::Large => (
                "stt.quota_accurate_daily_limit",
                "stt.quota_accurate_weekly_limit",
                "stt.quota_accurate_daily_limit",
                "stt.quota_accurate_weekly_limit",
                "stt.quota_accurate_file_too_long",
            ),
            SttModelSize::Small => (
                "stt.quota_fast_daily_limit",
                "stt.quota_fast_weekly_limit",
                "stt.quota_fast_daily_limit",
                "stt.quota_fast_weekly_limit",
                "stt.quota_fast_file_too_long",
            ),
        };

        if let Some(daily_limit) = daily_limit_opt {
            let daily_used = get_usage(db.client(), user_id, daily_kind, 86400).await.unwrap_or(0) as u64;
            let daily_remaining = daily_limit.saturating_sub(daily_used);
            if daily_remaining == 0 {
                remove_active_stt_job(user_id);
                log_trace(trace_id, "stt_quota_daily", &format!("user_id={user_id} kind={daily_key} used={daily_used} limit={daily_limit}"));
                let limit_str = format_duration_fa(daily_limit);
                let label = tf(daily_limit_key, &[("limit", &limit_str)]);
                delete_status(api, chat_id, status_msg_id).await;
                clean_up(&work_dir);
                let next = stt_next_rank(&user_rank);
                if let Some(min_rank) = next {
                    crate::rank::paywall::block_limit(api, chat_id, &label, min_rank).await;
                } else {
                    let _ = send_text(api, chat_id, &label).await;
                }
                return;
            }
            let weekly_limit = weekly_limit_opt.unwrap_or(u64::MAX);
            let weekly_used = get_usage(db.client(), user_id, weekly_kind, 7 * 86400).await.unwrap_or(0) as u64;
            let weekly_remaining = weekly_limit.saturating_sub(weekly_used);
            if weekly_remaining == 0 {
                remove_active_stt_job(user_id);
                log_trace(trace_id, "stt_quota_weekly", &format!("user_id={user_id} kind={weekly_key} used={weekly_used} limit={weekly_limit}"));
                let limit_str = format_duration_fa(weekly_limit);
                let label = tf(weekly_limit_key, &[("limit", &limit_str)]);
                delete_status(api, chat_id, status_msg_id).await;
                clean_up(&work_dir);
                let next = stt_next_rank(&user_rank);
                if let Some(min_rank) = next {
                    crate::rank::paywall::block_limit(api, chat_id, &label, min_rank).await;
                } else {
                    let _ = send_text(api, chat_id, &label).await;
                }
                return;
            }
            let remaining = daily_remaining.min(weekly_remaining);
            if duration_secs > remaining {
                remove_active_stt_job(user_id);
                log_trace(trace_id, "stt_quota_file_too_long", &format!("user_id={user_id} duration={duration_secs} remaining={remaining}"));
                let rem_str = format_duration_fa(remaining);
                let label = tf(file_key, &[("remaining", &rem_str)]);
                delete_status(api, chat_id, status_msg_id).await;
                clean_up(&work_dir);
                let next = stt_next_rank(&user_rank);
                if let Some(min_rank) = next {
                    crate::rank::paywall::block_limit(api, chat_id, &label, min_rank).await;
                } else {
                    let _ = send_text(api, chat_id, &label).await;
                }
                return;
            }
        }
    }

    // ── Stage 3: Optional denoise ──
    let denoise_secs = if config.denoise {
        edit_status(api, chat_id, status_msg_id, &t("stt.stage_denoising")).await;

        let wav_in = wav_str.to_string();
        let wav_out = denoised_str.to_string();
        match tokio::task::spawn_blocking(move || {
            deepfilter::denoise(&wav_in, &wav_out).map_err(|e| e.to_string())
        }).await.unwrap_or_else(|e| Err(e.to_string())) {
            Ok(s) => {
                log_trace(trace_id, "stt_denoised", &format!("elapsed={s:.1}s"));
                s
            }
            Err(e) => {
                log_trace(trace_id, "stt_denoise_failed", &format!("err={e}, falling back to raw"));
                let _ = std::fs::copy(&wav_path, &denoised_path);
                0.0
            }
        }
    } else {
        let _ = std::fs::copy(&wav_path, &denoised_path);
        0.0
    };

    if cancel_flag.load(Ordering::Relaxed) {
        log_trace(trace_id, "stt_cancelled_after_denoise", "");
        remove_active_stt_job(user_id);
        clean_up(&work_dir);
        return;
    }

    // ── Stage 4: Transcribe with live timer ──
    let duration_label = format_duration_hms(audio_duration);
    let initial_text = tf("stt.stage_transcribing", &[
        ("duration", &duration_label),
        ("elapsed", "0"),
    ]);
    edit_status(api, chat_id, status_msg_id, &initial_text).await;

    // Spawn live timer — edits status every 2 seconds
    let timer_running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let timer_flag = timer_running.clone();
    let cancel_timer_flag = cancel_flag.clone();
    let api_timer = api.clone();
    let timer_msg_id = status_msg_id;
    let dur_label = duration_label.clone();
    let timer_handle = tokio::spawn(async move {
        let mut tick = 2u64;
        while timer_flag.load(Ordering::Relaxed) && !cancel_timer_flag.load(Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if !timer_flag.load(Ordering::Relaxed) || cancel_timer_flag.load(Ordering::Relaxed) { break; }
            let text = tf("stt.stage_transcribing", &[
                ("duration", &dur_label),
                ("elapsed", &tick.to_string()),
            ]);
            edit_status(&api_timer, chat_id, timer_msg_id, &text).await;
            tick += 2;
        }
    });

    // 4. Transcribe — CPU-heavy blocking, run on thread pool
    let audio_source = if config.denoise {
        denoised_str.to_string()
    } else {
        wav_str.to_string()
    };
    let config_clone = config.clone();
    let (text, processing_secs) = match tokio::task::spawn_blocking(move || {
        vosk::transcribe(&config_clone, &audio_source).map_err(|e| e.to_string())
    }).await.unwrap_or_else(|e| Err(e.to_string())) {
        Ok(r) => r,
        Err(e) => {
            // Stop the timer
            timer_running.store(false, Ordering::Relaxed);
            let _ = timer_handle.await;
            remove_active_stt_job(user_id);
            log_trace(trace_id, "stt_transcribe_failed", &format!("err={e}"));
            crate::stats::record_event_user(user_id, "stt", &stt_action(config), "fail", duration_secs as i64).await;
            crate::stats::record_error_global("stt", &format!("transcribe failed: {e}")).await;
            let _ = send_text_with_back(api, chat_id, &t("stt.transcribe_failed")).await;
            delete_status(api, chat_id, status_msg_id).await;
            clean_up(&work_dir);
            return;
        }
    };

    // Stop the timer
    timer_running.store(false, Ordering::Relaxed);
    let _ = timer_handle.await;

    let was_cancelled = cancel_flag.load(Ordering::Relaxed);
    remove_active_stt_job(user_id);

    if was_cancelled {
        log_trace(trace_id, "stt_cancelled_during_transcribe", "");
        clean_up(&work_dir);
        return;
    }

    // Delete status message — result will be sent as a new message
    delete_status(api, chat_id, status_msg_id).await;

    log_trace(trace_id, "stt_transcribed", &format!("text_len={} elapsed={processing_secs:.1}s", text.len()));

    let lang_label = config.lang_label_fa();
    let model_label = config.model_label_fa();
    let denoise_label = if config.denoise { t("stt.denoise_on") } else { t("stt.denoise_off") };
    let total_secs = overall_start.elapsed().as_secs_f64();

    let result_text = tf("stt.result_report", &[
        ("lang", &lang_label),
        ("model", &model_label),
        ("denoise", &denoise_label),
        ("dur", &format!("{audio_duration:.1}")),
        ("total", &format!("{total_secs:.1}")),
        ("denoise_time", &format!("{denoise_secs:.1}")),
        ("text", &text),
    ]);

    // Use send_long_text — transcription can exceed Telegram's 4096-char limit for long audio
    let _ = send_long_text(api, chat_id, &result_text).await;
    log_trace(trace_id, "stt_result_sent", &format!("text_len={}", text.len()));
    crate::stats::record_event_user(user_id, "stt", &stt_action(config), "ok", duration_secs as i64).await;

    // ثبت مصرف quota
    if let Some(db) = database.as_ref() {
        let _ = add_usage(db.client(), user_id, daily_kind, duration_secs as i64, 86400).await;
        let _ = add_usage(db.client(), user_id, weekly_kind, duration_secs as i64, 7 * 86400).await;
        log_trace(trace_id, "stt_quota_added", &format!("user_id={user_id} secs={duration_secs}"));
    }

    clean_up(&work_dir);
}

fn wav_duration(path: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path])
        .output()?;
    if !output.status.success() {
        return Err(format!("ffprobe failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(s.parse()?)
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
            tf("rank.duration_hours_minutes", &[("hours", &hours.to_string()), ("mins", &rem_mins.to_string())])
        }
    }
}

fn stt_next_rank(rank: &rank::types::Rank) -> Option<rank::types::Rank> {
    match rank {
        rank::types::Rank::Dalavar | rank::types::Rank::Sepahbod | rank::types::Rank::Esfandyar => Some(rank::types::Rank::Sohrab),
        rank::types::Rank::Sohrab => Some(rank::types::Rank::Rostam),
        rank::types::Rank::Rostam => None,
    }
}

async fn edit_status(api: &Bot, chat_id: i64, message_id: Option<i32>, text: &str) {
    if let Some(msg_id) = message_id {
        let params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(msg_id)
            .text(text)
            .reply_markup(cancel_job_keyboard())
            .build();
        let _ = api.edit_message_text(&params).await;
    }
}

async fn delete_status(api: &Bot, chat_id: i64, message_id: Option<i32>) {
    if let Some(msg_id) = message_id {
        let params = frankenstein::methods::DeleteMessageParams::builder()
            .chat_id(chat_id)
            .message_id(msg_id)
            .build();
        let _ = api.delete_message(&params).await;
    }
}

fn format_duration_hms(secs_f: f64) -> String {
    let total_secs = secs_f.round() as u64;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    if mins > 0 {
        format!("{mins} دقیقه و {secs} ثانیه")
    } else {
        format!("{secs} ثانیه")
    }
}
