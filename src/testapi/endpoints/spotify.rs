//! TestAPI endpoint for Spotify downloader tests.

use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct SpotifyTrackReq {
    pub url: String,
    pub user_id: Option<i64>,
}

#[derive(Serialize)]
pub struct SpotifyTrackResp {
    pub ok: bool,
    pub detected_track_id: Option<String>,
    pub status_text: String,
    pub cancel_callback: String,
}

pub async fn test_spotify_download_track(
    Json(req): Json<SpotifyTrackReq>,
) -> Json<SpotifyTrackResp> {
    let track_id = crate::spotify::extract_spotify_track_id(&req.url);
    let ok = track_id.is_some();
    let status_text = if ok {
        crate::i18n::t("spotify.starting")
    } else {
        crate::i18n::t("spotify.track_not_found")
    };

    Json(SpotifyTrackResp {
        ok,
        detected_track_id: track_id,
        status_text,
        cancel_callback: crate::bot::CB_SP_CANCEL.to_string(),
    })
}

#[derive(Deserialize)]
pub struct SpotifyCancelReq {
    pub user_id: i64,
}

#[derive(Serialize)]
pub struct SpotifyCancelResp {
    pub ok: bool,
    pub user_id: i64,
    pub cancelled: bool,
}

pub async fn test_spotify_cancel(Json(req): Json<SpotifyCancelReq>) -> Json<SpotifyCancelResp> {
    let cancelled = crate::spotify::cancel_spotify_job(req.user_id);
    Json(SpotifyCancelResp {
        ok: true,
        user_id: req.user_id,
        cancelled,
    })
}
