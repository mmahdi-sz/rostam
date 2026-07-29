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
    pub available_disk_space_bytes: u64,
}

pub async fn test_surge_validate_url(
    Json(req): Json<SurgeValidateReq>,
) -> Json<SurgeValidateResp> {
    let valid = crate::surge_dl::is_direct_link(&req.url);
    let root = crate::config::surge_downloads_root();
    let available_disk_space_bytes =
        crate::surge_dl::available_disk_space(&root).unwrap_or(0);
    Json(SurgeValidateResp {
        ok: true,
        url: req.url,
        valid,
        available_disk_space_bytes,
    })
}
