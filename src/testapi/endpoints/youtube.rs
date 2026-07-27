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

pub async fn test_youtube_format(Json(req): Json<YoutubeFormatReq>) -> Json<YoutubeFormatResp> {
    let is_yt = req.url.contains("youtube.com") || req.url.contains("youtu.be");
    Json(YoutubeFormatResp {
        ok: is_yt,
        detected_url: req.url,
        title_sample: if is_yt { "Sample YouTube Title".into() } else { "Invalid URL".into() },
    })
}
