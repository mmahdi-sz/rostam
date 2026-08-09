use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct SttReq {
    pub file_id: String,
    pub lang: Option<String>,
}

#[derive(Serialize)]
pub struct SttResp {
    pub ok: bool,
    pub file_id: String,
    pub lang: String,
    pub text: String,
}

pub async fn test_stt_recognize(Json(req): Json<SttReq>) -> Json<SttResp> {
    let lang = req.lang.unwrap_or_else(|| "fa".to_string());
    Json(SttResp {
        ok: true,
        file_id: req.file_id,
        lang: lang.clone(),
        text: format!("[STT mock transcript in {lang}]"),
    })
}

#[derive(Deserialize)]
pub struct SeparationReq {
    pub file_id: String,
    pub mode: Option<String>,
}

#[derive(Serialize)]
pub struct SeparationResp {
    pub ok: bool,
    pub file_id: String,
    pub mode: String,
    pub job_id: u64,
}

pub async fn test_separation_submit(Json(req): Json<SeparationReq>) -> Json<SeparationResp> {
    let mode = req.mode.unwrap_or_else(|| "stems2".to_string());
    Json(SeparationResp {
        ok: true,
        file_id: req.file_id,
        mode,
        job_id: 1001,
    })
}

#[derive(Deserialize)]
pub struct GwmReq {
    pub file_id: String,
}

#[derive(Serialize)]
pub struct GwmResp {
    pub ok: bool,
    pub file_id: String,
    pub watermark_detected: bool,
    pub confidence: f32,
}

pub async fn test_gwm_detect(Json(req): Json<GwmReq>) -> Json<GwmResp> {
    Json(GwmResp {
        ok: true,
        file_id: req.file_id,
        watermark_detected: false,
        confidence: 0.12,
    })
}

#[derive(Deserialize)]
pub struct DenoiseReq {
    pub file_id: String,
    pub is_video: Option<bool>,
}

#[derive(Serialize)]
pub struct DenoiseResp {
    pub ok: bool,
    pub file_id: String,
    pub is_video: bool,
    pub result_caption: String,
}

pub async fn test_denoise_process(Json(req): Json<DenoiseReq>) -> Json<DenoiseResp> {
    let is_video = req.is_video.unwrap_or(false);
    let result_caption = crate::i18n::t("denoise.result_caption");
    Json(DenoiseResp {
        ok: true,
        file_id: req.file_id,
        is_video,
        result_caption,
    })
}

#[derive(Deserialize)]
pub struct TtsReq {
    pub text: String,
    pub mode: Option<String>,
}

#[derive(Serialize)]
pub struct TtsResp {
    pub ok: bool,
    pub text: String,
    pub mode: String,
    pub result_caption: String,
    /// Extension of the produced file — `ogg` means the ffmpeg/libopus
    /// conversion succeeded, which is what silently broke in 2.1.3.
    pub output_ext: Option<String>,
    pub output_bytes: Option<u64>,
    pub err: Option<String>,
}

/// Calls the real synthesis engine (`moss_tts::engine::run_tts_engine`), so a
/// broken Piper model or a failing ffmpeg conversion shows up here instead of
/// only in production.
pub async fn test_tts_generate(Json(req): Json<TtsReq>) -> Json<TtsResp> {
    let mode = req.mode.unwrap_or_else(|| "default".to_string());
    let result_caption = crate::i18n::t("tts.result_caption");
    let trace_id = crate::log::next_trace_id();

    // Drain the progress channel so the engine never blocks on a full queue.
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let out =
        crate::moss_tts::engine::run_tts_engine(&req.text, -999_001, trace_id, tx, cancel).await;

    let (ok, output_ext, output_bytes, err) = match out {
        Ok(path) => {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_string())
                .unwrap_or_default();
            let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let _ = std::fs::remove_file(&path);
            (bytes > 0 && ext == "ogg", Some(ext), Some(bytes), None)
        }
        Err(e) => (false, None, None, Some(e)),
    };

    Json(TtsResp {
        ok,
        text: req.text,
        mode,
        result_caption,
        output_ext,
        output_bytes,
        err,
    })
}

#[derive(Deserialize)]
pub struct DeoldifyReq {
    pub file_id: String,
    pub render_factor: Option<u32>,
}

#[derive(Serialize)]
pub struct DeoldifyResp {
    pub ok: bool,
    pub file_id: String,
    pub render_factor: u32,
    pub result_caption: String,
}

pub async fn test_deoldify_colorize(Json(req): Json<DeoldifyReq>) -> Json<DeoldifyResp> {
    let render_factor = req.render_factor.unwrap_or(24);
    let result_caption = crate::i18n::t("deoldify.result_caption");
    Json(DeoldifyResp {
        ok: true,
        file_id: req.file_id,
        render_factor,
        result_caption,
    })
}

#[derive(Deserialize)]
pub struct NobgReq {
    pub file_id: String,
}

#[derive(Serialize)]
pub struct NobgResp {
    pub ok: bool,
    pub file_id: String,
    pub result_caption: String,
}

pub async fn test_nobg_process(Json(req): Json<NobgReq>) -> Json<NobgResp> {
    let result_caption = crate::i18n::tf("nobg.result_caption", &[("time", "1.2")]);
    Json(NobgResp {
        ok: true,
        file_id: req.file_id,
        result_caption,
    })
}

// ── TTS UX surface (character limit / cancel keyboard) ────────────────────────

#[derive(Deserialize)]
pub struct TtsUxReq {
    /// Input text length in characters; for testing the long text path.
    pub char_len: Option<usize>,
}

#[derive(Serialize)]
pub struct TtsUxButton {
    pub text: String,
    pub callback_data: String,
    pub style: String,
}

#[derive(Serialize)]
pub struct TtsUxResp {
    pub ok: bool,
    pub max_chars: usize,
    pub char_len: usize,
    /// Whether limit was exceeded and text should be rejected.
    pub too_long: bool,
    /// Limit error message text; only when too_long is true.
    pub too_long_text: Option<String>,
    pub progress_keyboard: Vec<Vec<TtsUxButton>>,
    pub ask_text_keyboard: Vec<Vec<TtsUxButton>>,
    pub cancelled_text: String,
}

fn dump_tts(kbd: &frankenstein::types::InlineKeyboardMarkup) -> Vec<Vec<TtsUxButton>> {
    kbd.inline_keyboard
        .iter()
        .map(|row| {
            row.iter()
                .map(|b| TtsUxButton {
                    text: b.text.clone(),
                    callback_data: b.callback_data.clone().unwrap_or_default(),
                    style: match b.style {
                        Some(frankenstein::types::ButtonStyle::Success) => "success",
                        Some(frankenstein::types::ButtonStyle::Primary) => "primary",
                        Some(frankenstein::types::ButtonStyle::Danger) => "danger",
                        _ => "default",
                    }
                    .to_string(),
                })
                .collect()
        })
        .collect()
}

pub async fn test_tts_ux(Json(req): Json<TtsUxReq>) -> Json<TtsUxResp> {
    let max_chars = crate::moss_tts::TTS_MAX_CHARS;
    let char_len = req.char_len.unwrap_or(10);
    let too_long = char_len > max_chars;
    let too_long_text = if too_long {
        Some(crate::i18n::tf(
            "tts.text_too_long",
            &[
                ("len", &char_len.to_string()),
                ("max", &max_chars.to_string()),
            ],
        ))
    } else {
        None
    };

    Json(TtsUxResp {
        ok: true,
        max_chars,
        char_len,
        too_long,
        too_long_text,
        progress_keyboard: dump_tts(&crate::moss_tts::tts_job_cancel_keyboard_for_test()),
        ask_text_keyboard: dump_tts(&crate::moss_tts::tts_cancel_keyboard_for_test()),
        cancelled_text: crate::i18n::t("tts.cancelled"),
    })
}

// ── STT ready surface (model label + premium emoji) ───────────────────────────

#[derive(Deserialize)]
pub struct SttReadyReq {
    /// One of fa_big / fa_small / en_big / en_small
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct SttReadyResp {
    pub ok: bool,
    pub model: String,
    /// Human-readable inline button label — not an i18n key.
    pub model_label: String,
    /// Final text after MarkdownV2 + premium emoji replacement.
    pub ready_title: String,
    pub ready_again: String,
    /// Count of rendered premium emojis in text.
    pub premium_emoji_count: usize,
}

pub async fn test_stt_ready(Json(req): Json<SttReadyReq>) -> Json<SttReadyResp> {
    use crate::stt::types::{SttConfig, SttLang, SttModelSize};

    let model = req.model.unwrap_or_else(|| "fa_small".to_string());
    let (lang, size) = match model.as_str() {
        "fa_big" => (SttLang::Fa, SttModelSize::Large),
        "en_big" => (SttLang::En, SttModelSize::Large),
        "en_small" => (SttLang::En, SttModelSize::Small),
        _ => (SttLang::Fa, SttModelSize::Small),
    };
    let config = SttConfig {
        lang,
        model_size: size,
        denoise: false,
    };

    let model_label = crate::i18n::t(config.label_key());
    let escaped = crate::i18n::md_escape(&model_label);
    let ready_title = crate::i18n::apply_premium_to_md(&crate::i18n::tf(
        "stt.ready_title",
        &[("model", &escaped)],
    ));
    let ready_again = crate::i18n::apply_premium_to_md(&crate::i18n::tf(
        "stt.ready_again",
        &[("model", &escaped)],
    ));
    let premium_emoji_count = ready_title.matches("tg://emoji?id=").count();

    Json(SttReadyResp {
        ok: true,
        model,
        model_label,
        ready_title,
        ready_again,
        premium_emoji_count,
    })
}
