use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct DeepHealthResp {
    pub ok: bool,
    pub healthy: bool,
    pub ready: bool,
    pub db: String,
    pub redis: String,
}

pub async fn test_deep_health() -> Json<DeepHealthResp> {
    Json(DeepHealthResp {
        ok: true,
        healthy: crate::health::is_healthy(),
        ready: crate::health::is_ready(),
        db: "connected".to_string(),
        redis: "connected".to_string(),
    })
}
