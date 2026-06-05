use std::path::PathBuf;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendAudioParams, SendVoiceParams},
    types::{InlineKeyboardMarkup, Message},
};

use crate::bot::{send_text, send_text_md, CB_DENOISE_CANCEL};
use crate::emoji::{FlowManager, FlowState};
use crate::emoji::panel::btn_icon_danger;
use crate::i18n::{t, tf, apply_premium_to_md};
use crate::stt::deepfilter;
use crate::youtube::log_trace;

fn next_trace_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
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

    let text = apply_premium_to_md(&t("denoise.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(denoise_keyboard())
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => log_trace(trace_id, "denoise_prompt_shown", &format!("user_id={user_id} chat_id={chat_id}")),
        Err(e) => log_trace(trace_id, "denoise_prompt_failed", &e.to_string()),
    }
}

fn denoise_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon_danger(&t("denoise.cancel_button"), CB_DENOISE_CANCEL, "cancel")],
        ])
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
    let r = crate::bot::edit_to_ai_lab(api, chat_id, message_id).await;
    log_trace(next_trace_id(), "denoise_cancel_done", &format!("ok={}", r.is_ok()));
}

/// Processes an audio message when user is in AwaitingDenoiseAudio.
pub async fn handle_denoise_audio(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    // Clear denoise flow state — user sent audio, processing begins
    flow_manager.clear(user_id);
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

    let is_voice = message.voice.is_some();
    let is_audio = message.audio.is_some();
    let is_doc = message.document.is_some();
    let orig_ext = detect_format(message);

    // Extract original filename for output naming
    let orig_stem = message
        .audio.as_ref().and_then(|a| a.file_name.as_deref())
        .or_else(|| message.document.as_ref().and_then(|d| d.file_name.as_deref()))
        .and_then(|name| {
            let dot = name.rfind('.')?;
            Some(&name[..dot])
        })
        .unwrap_or("voice");
    let clean_filename = format!("{orig_stem}_clean.{orig_ext}");

    log_trace(trace_id, "denoise_audio_received", &format!(
        "user_id={user_id} chat_id={chat_id} voice={is_voice} audio={is_audio} doc={is_doc} ext={orig_ext} stem={orig_stem} clean={clean_filename}"
    ));

    let _ = send_text(api, chat_id, &t("denoise.preparing")).await;

    let work_dir = std::env::temp_dir().join(format!("denoise_{trace_id}"));
    std::fs::create_dir_all(&work_dir).ok();

    let input_path = work_dir.join(format!("input.{}", orig_ext));
    let wav_path = work_dir.join("denoise_input.wav");
    let denoised_path = work_dir.join("denoised.wav");
    let output_path = work_dir.join(&clean_filename);

    // 1. Download
    if let Err(e) = download_file(api, file_id, input_path.to_str().unwrap()).await {
        log_trace(trace_id, "denoise_download_failed", &format!("err={e}"));
        let _ = send_text(api, chat_id, &t("denoise.download_failed")).await;
        clean_up(&work_dir);
        return;
    }
    let file_size = std::fs::metadata(input_path.to_str().unwrap()).map(|m| m.len()).unwrap_or(0);
    log_trace(trace_id, "denoise_downloaded", &format!("size={file_size}"));

    // 2. Convert to 48kHz mono 16-bit PCM WAV (DeepFilterNet optimal sample rate)
    if let Err(e) = convert_to_wav(input_path.to_str().unwrap(), wav_path.to_str().unwrap(), 48000) {
        log_trace(trace_id, "denoise_convert_failed", &format!("err={e}"));
        let _ = send_text(api, chat_id, &t("denoise.convert_failed")).await;
        clean_up(&work_dir);
        return;
    }
    log_trace(trace_id, "denoise_converted", "");

    // Determine audio duration from WAV header
    let audio_duration = wav_duration(wav_path.to_str().unwrap()).unwrap_or(0.0);

    // 3. Denoise via DeepFilterNet
    let processing_secs = match deepfilter::denoise(wav_path.to_str().unwrap(), denoised_path.to_str().unwrap()) {
        Ok(s) => {
            log_trace(trace_id, "denoise_done", &format!("elapsed={s:.1}s"));
            s
        }
        Err(e) => {
            log_trace(trace_id, "denoise_failed", &format!("err={e}"));
            let _ = send_text(api, chat_id, &t("denoise.denoise_failed")).await;
            clean_up(&work_dir);
            return;
        }
    };

    // 4. Convert back to original format
    if let Err(e) = convert_from_wav(denoised_path.to_str().unwrap(), output_path.to_str().unwrap(), &orig_ext) {
        log_trace(trace_id, "denoise_reconvert_failed", &format!("err={e}"));
        let _ = send_text(api, chat_id, &t("denoise.convert_failed")).await;
        clean_up(&work_dir);
        return;
    }
    log_trace(trace_id, "denoise_reconverted", &format!("ext={orig_ext}"));

    // 5. Send denoised file
    let efficiency = if processing_secs > 0.0 { audio_duration / processing_secs } else { 0.0 };

    let caption = apply_premium_to_md(&t("denoise.result_caption"));

    if is_voice {
        let params = SendVoiceParams::builder()
            .chat_id(chat_id)
            .voice(PathBuf::from(output_path.to_str().unwrap()))
            .caption(&caption)
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        let r = api.send_voice(&params).await;
        log_trace(trace_id, "denoise_voice_sent", &format!("ok={}", r.is_ok()));
    } else {
        let params = SendAudioParams::builder()
            .chat_id(chat_id)
            .audio(PathBuf::from(output_path.to_str().unwrap()))
            .caption(&caption)
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        let r = api.send_audio(&params).await;
        log_trace(trace_id, "denoise_audio_sent", &format!("ok={}", r.is_ok()));
    }

    // 6. Send report
    let duration_str = escape_md(&format!("{:.1}", audio_duration));
    let processing_str = escape_md(&format!("{:.1}", processing_secs));
    let ratio_str = escape_md(&format!("{:.1}", efficiency));
    let report = apply_premium_to_md(&tf("denoise.report", &[
        ("duration", &duration_str),
        ("processing", &processing_str),
        ("ratio", &ratio_str),
    ]));
    let _ = send_text_md(api, chat_id, &report).await;
    log_trace(trace_id, "denoise_report_sent", &format!("duration={audio_duration:.1}s processing={processing_secs:.1}s"));

    clean_up(&work_dir);
}

fn detect_format(message: &Message) -> String {
    if message.voice.is_some() {
        return "ogg".to_string();
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
        "audio/mp4" | "audio/aac" => "m4a",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/wav" | "audio/wave" => "wav",
        "audio/flac" => "flac",
        "audio/webm" => "webm",
        _ => "wav",
    }.to_string()
}

fn convert_to_wav(input: &str, output: &str, sample_rate: u32) -> Result<(), Box<dyn std::error::Error>> {
    let output = std::process::Command::new("ffmpeg")
        .args([
            "-y", "-i", input,
            "-ar", &sample_rate.to_string(), "-ac", "1", "-sample_fmt", "s16",
            "-f", "wav", output,
        ])
        .output()
        .map_err(|e| format!("ffmpeg spawn failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg conversion failed: {stderr}").into());
    }
    Ok(())
}

fn convert_from_wav(input: &str, output: &str, ext: &str) -> Result<(), Box<dyn std::error::Error>> {
    let status = match ext {
        "ogg" => std::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "libopus", "-b:a", "32k", output])
            .status()
            .map_err(|e| format!("ffmpeg failed: {e}"))?,
        "mp3" => std::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "libmp3lame", "-b:a", "128k", output])
            .status()
            .map_err(|e| format!("ffmpeg failed: {e}"))?,
        "m4a" => std::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "aac", "-b:a", "128k", output])
            .status()
            .map_err(|e| format!("ffmpeg failed: {e}"))?,
        "flac" => std::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "flac", output])
            .status()
            .map_err(|e| format!("ffmpeg failed: {e}"))?,
        "webm" => std::process::Command::new("ffmpeg")
            .args(["-y", "-i", input, "-c:a", "libopus", output])
            .status()
            .map_err(|e| format!("ffmpeg failed: {e}"))?,
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
        return Err("ffmpeg reconversion failed".into());
    }
    Ok(())
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

async fn download_file(api: &Bot, file_id: &str, dest: &str) -> Result<(), Box<dyn std::error::Error>> {
    use frankenstein::methods::GetFileParams;

    let file_info = api.get_file(&GetFileParams::builder().file_id(file_id).build()).await?;
    let file_path = file_info.result.file_path.ok_or("no file_path")?;

    log_trace(next_trace_id(), "denoise_file_path", &format!("file_path={file_path}"));

    // Local Bot API returns an absolute filesystem path in --local mode.
    // In that case, copy directly from the filesystem.
    if file_path.starts_with('/') {
        std::fs::copy(&file_path, dest)?;
        log_trace(next_trace_id(), "denoise_file_local_copy", &format!("size={}", std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0)));
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

/// Escape MarkdownV2 special characters in dynamic text.
/// Does NOT touch `*` since those may be formatting markers in the i18n template.
fn escape_md(s: &str) -> String {
    s.chars().map(|c| match c {
        '_' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|' | '{' | '}' | '.' | '!' => {
            format!("\\{c}")
        }
        other => other.to_string(),
    }).collect()
}

fn clean_up(dir: &std::path::Path) {
    std::fs::remove_dir_all(dir).ok();
}
