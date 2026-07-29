use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct YoutubeFormatReq {
    pub url: String,
}

#[derive(Serialize)]
pub struct YoutubeFormatResp {
    pub ok: bool,
    pub detected_url: String,
    pub title_sample: String,
}

pub async fn test_youtube_format(
    Json(req): Json<YoutubeFormatReq>,
) -> (axum::http::StatusCode, Json<YoutubeFormatResp>) {
    let is_yt = req.url.contains("youtube.com") || req.url.contains("youtu.be");
    (
        axum::http::StatusCode::OK,
        Json(YoutubeFormatResp {
            ok: is_yt,
            detected_url: req.url,
            title_sample: if is_yt {
                "Sample YouTube Title".into()
            } else {
                "Invalid URL".into()
            },
        }),
    )
}

#[derive(Deserialize)]
pub struct QualitySelectReq {
    pub request_id: u64,
    pub height: u32,
}

#[derive(Serialize)]
pub struct QualitySelectResp {
    pub ok: bool,
    pub request_id: u64,
    pub height: u32,
    pub callback_data: String,
}

pub async fn test_youtube_quality_select(
    Json(req): Json<QualitySelectReq>,
) -> Json<QualitySelectResp> {
    let cb = format!("yt:q:{}:{}", req.request_id, req.height);
    Json(QualitySelectResp {
        ok: true,
        request_id: req.request_id,
        height: req.height,
        callback_data: cb,
    })
}

#[derive(Deserialize)]
pub struct CancelReq {
    pub request_id: u64,
}

#[derive(Serialize)]
pub struct CancelResp {
    pub ok: bool,
    pub request_id: u64,
    pub cancelled: bool,
}

pub async fn test_youtube_cancel(Json(req): Json<CancelReq>) -> Json<CancelResp> {
    let cancelled = crate::youtube::download::cancel_download(req.request_id);
    Json(CancelResp {
        ok: true,
        request_id: req.request_id,
        cancelled,
    })
}
