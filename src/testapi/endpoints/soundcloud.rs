//! TestAPI endpoint for SoundCloud downloader tests.

use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct SoundcloudTrackReq {
    pub url: String,
    pub user_id: Option<i64>,
}

#[derive(Serialize)]
pub struct SoundcloudTrackResp {
    pub ok: bool,
    pub detected_url: Option<String>,
    pub status_text: String,
    pub cancel_callback: String,
}

pub async fn test_soundcloud_download_track(
    Json(req): Json<SoundcloudTrackReq>,
) -> Json<SoundcloudTrackResp> {
    let sc_url = crate::soundcloud::extract_soundcloud_url(&req.url);
    let ok = sc_url.is_some();
    let status_text = if ok {
        crate::i18n::t("soundcloud.starting")
    } else {
        crate::i18n::t("soundcloud.track_not_found")
    };

    Json(SoundcloudTrackResp {
        ok,
        detected_url: sc_url,
        status_text,
        cancel_callback: crate::bot::CB_SC_CANCEL.to_string(),
    })
}

#[derive(Deserialize)]
pub struct SoundcloudCancelReq {
    pub user_id: i64,
}

#[derive(Serialize)]
pub struct SoundcloudCancelResp {
    pub ok: bool,
    pub user_id: i64,
    pub cancelled: bool,
}

pub async fn test_soundcloud_cancel(
    Json(req): Json<SoundcloudCancelReq>,
) -> Json<SoundcloudCancelResp> {
    let cancelled = crate::soundcloud::cancel_soundcloud_job(req.user_id);
    Json(SoundcloudCancelResp {
        ok: true,
        user_id: req.user_id,
        cancelled,
    })
}
