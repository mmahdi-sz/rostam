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
