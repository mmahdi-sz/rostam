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

    let out = crate::moss_tts::engine::run_tts_engine(&req.text, -999_001, trace_id, tx).await;

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
