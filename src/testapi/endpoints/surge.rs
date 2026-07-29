use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct SurgeValidateReq {
    pub url: String,
}

#[derive(Serialize)]
pub struct SurgeValidateResp {
    pub ok: bool,
    pub url: String,
    pub valid: bool,
    pub safe: bool,
}

pub async fn test_surge_validate_url(Json(req): Json<SurgeValidateReq>) -> Json<SurgeValidateResp> {
    let valid = req.url.starts_with("http://") || req.url.starts_with("https://");
    let safe = valid && !req.url.contains("127.0.0.1") && !req.url.contains("localhost");
    Json(SurgeValidateResp {
        ok: true,
        url: req.url,
        valid,
        safe,
    })
}
