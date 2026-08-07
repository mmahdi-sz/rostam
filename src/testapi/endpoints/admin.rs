use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct AdminPanelReq {
    pub user_id: i64,
}

#[derive(Serialize)]
pub struct AdminPanelResp {
    pub ok: bool,
    pub is_admin: bool,
    pub rendered_panel: String,
}

pub async fn test_admin_panel(Json(req): Json<AdminPanelReq>) -> Json<AdminPanelResp> {
    let admin_id = crate::config::admin_user_id().unwrap_or(12345);
    let is_admin = req.user_id == admin_id;
    Json(AdminPanelResp {
        ok: true,
        is_admin,
        rendered_panel: if is_admin {
            "📊 Admin Panel: Stats and Controls".to_string()
        } else {
            "Access Denied".to_string()
        },
    })
}

#[derive(Deserialize)]
pub struct AdminBroadcastReq {
    pub mode: String,
    pub pin: bool,
    pub target_count: Option<i64>,
}

#[derive(Serialize)]
pub struct AdminBroadcastResp {
    pub ok: bool,
    pub mode: String,
    pub pin: bool,
    pub target_count: i64,
    pub simulated_success: i64,
    pub rendered_text: String,
}

pub async fn test_admin_broadcast(Json(req): Json<AdminBroadcastReq>) -> Json<AdminBroadcastResp> {
    let target = req.target_count.unwrap_or(100);
    let broadcast_mode = if req.mode.to_lowercase() == "forward" {
        crate::emoji::flow::BroadcastMode::Forward
    } else {
        crate::emoji::flow::BroadcastMode::Copy
    };
    let rendered_text = crate::admin::broadcast::format_broadcast_status(
        broadcast_mode,
        target as usize,
        target as usize,
        60,
    );
    Json(AdminBroadcastResp {
        ok: true,
        mode: req.mode,
        pin: req.pin,
        target_count: target,
        simulated_success: target,
        rendered_text,
    })
}
