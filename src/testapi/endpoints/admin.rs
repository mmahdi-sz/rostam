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
pub struct AdminStatsSectionReq {
    /// Section key (`ov`, `users`, `yt`, `ai`, `music`, `files`, `money`, `sys`, `err`).
    pub section: String,
}

#[derive(Serialize)]
pub struct AdminStatsSectionResp {
    pub ok: bool,
    pub section: String,
    pub known_section: bool,
    pub db: bool,
    pub html: bool,
    pub rendered_text: String,
    pub nav_labels: Vec<String>,
    pub nav_callbacks: Vec<String>,
    pub current_marked: bool,
}

/// Renders one stats page through the real `admin::render_section` + nav keyboard.
/// Without a DB (or with an unknown key) it still returns the keyboard, so the
/// failure path is observable.
pub async fn test_admin_stats_section(
    Json(req): Json<AdminStatsSectionReq>,
) -> Json<AdminStatsSectionResp> {
    let database = crate::testapi::state::db().await;
    let known_section = crate::admin::section(&req.section).is_some();
    let (rendered_text, html) = match &database {
        Some(db) => {
            if let Ok(client) = db.get().await {
                let view = crate::admin::render_section(&client, &req.section).await;
                (view.text, view.html)
            } else {
                (crate::i18n::t("admin.db_missing"), false)
            }
        }
        None => (crate::i18n::t("admin.db_missing"), false),
    };
    let kb = crate::admin::stats_keyboard(&req.section);
    let mut nav_labels = Vec::new();
    let mut nav_callbacks = Vec::new();
    let mut current_marked = false;
    let want = format!("{}{}", crate::bot::constants::CB_ADMIN_SECTION, req.section);
    for row in &kb.inline_keyboard {
        for btn in row {
            nav_labels.push(btn.text.clone());
            let cb = btn.callback_data.clone().unwrap_or_default();
            if cb == want {
                current_marked = btn.style.is_some();
            }
            nav_callbacks.push(cb);
        }
    }
    Json(AdminStatsSectionResp {
        ok: true,
        section: req.section,
        known_section,
        db: database.is_some(),
        html,
        rendered_text,
        nav_labels,
        nav_callbacks,
        current_marked,
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
