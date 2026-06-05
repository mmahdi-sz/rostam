use std::time::Instant;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{AnswerCallbackQueryParams, EditMessageTextParams, SendMessageParams},
    types::{
        ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, LinkPreviewOptions,
        MaybeInaccessibleMessage, Message, ReplyMarkup,
    },
};

use crate::bot::{edit_to_start_menu, send_text, CB_START_PANEL};
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{entities_for_text, t, tf};
use crate::stt::config::*;
use crate::stt::deepfilter;
use crate::stt::types::{SttConfig, SttLang, SttModelSize};
use crate::stt::vosk;
use crate::youtube::log_trace;

fn next_trace_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Called from main.rs when `ai:stt` is pressed.
/// Edits the AI Lab submenu message to show the STT config menu.
pub async fn enter_stt_config(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let config = SttConfig {
        lang: SttLang::Fa,
        model_size: SttModelSize::Large,
        denoise: true,
    };
    flow_manager.set(user_id, FlowState::AwaitingSttConfig { config: config.clone() });

    let text = t("stt.config_title");
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .reply_markup(config_keyboard(true))
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => log_trace(trace_id, "stt_config_shown", &format!("user_id={user_id} chat_id={chat_id}")),
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
) -> bool {
    let trace_id = next_trace_id();

    match data {
        CB_STT_FA_BIG | CB_STT_FA_SMALL | CB_STT_EN_BIG | CB_STT_EN_SMALL => {
            let (lang, size) = match data {
                CB_STT_FA_BIG => (SttLang::Fa, SttModelSize::Large),
                CB_STT_FA_SMALL => (SttLang::Fa, SttModelSize::Small),
                CB_STT_EN_BIG => (SttLang::En, SttModelSize::Large),
                CB_STT_EN_SMALL => (SttLang::En, SttModelSize::Small),
                _ => unreachable!(),
            };

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

/// Downloads a Telegram file by file_id to a local path.
async fn download_file(api: &Bot, file_id: &str, dest: &str) -> Result<(), Box<dyn std::error::Error>> {
    use frankenstein::methods::GetFileParams;

    let file_info = api.get_file(&GetFileParams::builder().file_id(file_id).build()).await?;
    let file_path = file_info.result.file_path.ok_or("no file_path")?;

    // Local Bot API returns an absolute filesystem path in --local mode.
    // In that case, copy directly from the filesystem.
    if file_path.starts_with('/') {
        std::fs::copy(&file_path, dest)?;
        return Ok(());
    }

    let url = if let Some(base) = crate::config::bot_api_base_url() {
        let base = base.trim_end_matches('/');
        format!("{base}/file/bot{}/{file_path}", crate::config::bot_token()?)
    } else {
        format!("https://api.telegram.org/file/bot{}/{file_path}", crate::config::bot_token()?)
    };

    let response = reqwest::get(&url).await?;
    let bytes = response.bytes().await?;
    std::fs::write(dest, &bytes)?;
    Ok(())
}

/// Processes an audio message (voice or audio file) when the user is in AwaitingSttAudio.
pub async fn handle_stt_audio(
    api: &Bot,
    message: &Message,
    user_id: i64,
    config: &SttConfig,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    let file_id = message
        .voice
        .as_ref()
        .map(|v| &v.file_id)
        .or_else(|| message.audio.as_ref().map(|a| &a.file_id))
        .or_else(|| message.document.as_ref().map(|d| &d.file_id));

    let Some(file_id) = file_id else {
        let _ = send_text(api, chat_id, &t("stt.unsupported_format")).await;
        return;
    };

    log_trace(trace_id, "stt_audio_received", &format!("user_id={user_id} chat_id={chat_id}"));

    let _ = send_text(api, chat_id, &t("stt.preparing")).await;

    let work_dir = std::env::temp_dir().join(format!("stt_{trace_id}"));
    std::fs::create_dir_all(&work_dir).ok();

    let input_path = work_dir.join("input");
    let wav_path = work_dir.join("converted.wav");
    let denoised_path = work_dir.join("denoised.wav");

    let overall_start = Instant::now();

    // 1. Download
    if let Err(e) = download_file(api, file_id, input_path.to_str().unwrap()).await {
        log_trace(trace_id, "stt_download_failed", &format!("err={e}"));
        let _ = send_text(api, chat_id, &t("stt.download_failed")).await;
        clean_up(&work_dir);
        return;
    }
    log_trace(trace_id, "stt_downloaded", "");

    // 2. Convert to WAV
    if let Err(e) = convert_to_wav(input_path.to_str().unwrap(), wav_path.to_str().unwrap()) {
        log_trace(trace_id, "stt_convert_failed", &format!("err={e}"));
        let _ = send_text(api, chat_id, &t("stt.convert_failed")).await;
        clean_up(&work_dir);
        return;
    }
    log_trace(trace_id, "stt_converted", "");

    // Determine audio duration from WAV header
    let audio_duration = wav_duration(wav_path.to_str().unwrap()).unwrap_or(0.0);

    // 3. Optional denoise
    let denoise_secs = if config.denoise {
        match deepfilter::denoise(wav_path.to_str().unwrap(), denoised_path.to_str().unwrap()) {
            Ok(s) => {
                log_trace(trace_id, "stt_denoised", &format!("elapsed={s:.1}s"));
                s
            }
            Err(e) => {
                log_trace(trace_id, "stt_denoise_failed", &format!("err={e}, falling back to raw"));
                // Fall back to raw audio
                let _ = std::fs::copy(&wav_path, &denoised_path);
                0.0
            }
        }
    } else {
        let _ = std::fs::copy(&wav_path, &denoised_path);
        0.0
    };

    // 4. Transcribe
    let audio_source = if config.denoise { denoised_path.to_str().unwrap() } else { wav_path.to_str().unwrap() };
    let (text, processing_secs) = match vosk::transcribe(config, audio_source) {
        Ok(r) => r,
        Err(e) => {
            log_trace(trace_id, "stt_transcribe_failed", &format!("err={e}"));
            let _ = send_text(api, chat_id, &t("stt.transcribe_failed")).await;
            clean_up(&work_dir);
            return;
        }
    };

    log_trace(trace_id, "stt_transcribed", &format!("text={text:?} elapsed={processing_secs:.1}s"));

    // 5. Build result message
    let lang_label = config.lang_label_fa();
    let model_label = config.model_label_fa();
    let denoise_label = if config.denoise { "فعال" } else { "غیرفعال" };
    let total_secs = overall_start.elapsed().as_secs_f64();

    let result_text = format!(
        "مشخصات رونویسی:\n\nزبان: {lang}\nمدل: {model}\nبهبود خودکار صدا: {denoise}\nمدت صدا: {dur:.1} ثانیه\nزمان پردازش: {total:.1} ثانیه\nزمان نویزگیری: {denoise_time:.1} ثانیه\n\nرونویسی {lang} ({model}) — انجام شد.\n\n{text}",
        lang = lang_label,
        model = model_label,
        denoise = denoise_label,
        dur = audio_duration,
        total = total_secs,
        denoise_time = denoise_secs,
        text = text,
    );

    let _ = send_text(api, chat_id, &result_text).await;
    log_trace(trace_id, "stt_result_sent", &format!("text_len={}", text.len()));

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
