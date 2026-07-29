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
