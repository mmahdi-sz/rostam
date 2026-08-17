use crate::bot::messaging::CAPTURED_EMOJIS;
use crate::bot::messaging::expand_and_entify_for_test;
use crate::log::CAPTURED_TRACES;
use axum::Json;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub async fn test_premium_render(Json(payload): Json<Value>) -> axum::response::Response {
    let text = match payload.get("text") {
        Some(v) if v.is_string() => v.as_str().unwrap().to_string(),
        Some(_) => {
            return (axum::http::StatusCode::BAD_REQUEST, "text must be a string").into_response();
        }
        None => return (axum::http::StatusCode::BAD_REQUEST, "missing text").into_response(),
    };

    let chat_id = match payload.get("chat_id") {
        Some(v) if v.is_i64() => v.as_i64().unwrap(),
        Some(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "chat_id must be an integer",
            )
                .into_response();
        }
        None => 12345,
    };

    let emojis = Arc::new(Mutex::new(Vec::new()));
    let traces = Arc::new(Mutex::new(Vec::new()));

    let e = emojis.clone();
    let t = traces.clone();

    let (rendered, entities, _) = CAPTURED_TRACES
        .scope(t, async {
            CAPTURED_EMOJIS
                .scope(e, async {
                    expand_and_entify_for_test(&text, chat_id).await
                })
                .await
        })
        .await;

    Json(json!({
        "ok": true,
        "rendered_text": rendered,
        "entities": entities.iter().map(|e| {
            json!({
                "type": e.type_field,
                "offset": e.offset,
                "length": e.length,
                "custom_emoji_id": e.custom_emoji_id,
            })
        }).collect::<Vec<_>>(),
        "custom_emoji_spans": emojis.lock().unwrap().clone(),
        "trace": traces.lock().unwrap().clone(),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct EmojiPanelReq {
    pub user_id: i64,
}

#[derive(Serialize)]
pub struct EmojiPanelResp {
    pub ok: bool,
    /// `/emoji` only renders the panel for the bot admin.
    pub is_admin: bool,
    pub opens_panel: bool,
    pub rendered_text: String,
    pub button_callbacks: Vec<String>,
    /// The panel must stay out of every user-facing menu.
    pub in_start_menu: bool,
}

/// `/emoji` admin gate: the real `emoji::panel` render for the admin, nothing for anyone else.
pub async fn test_emoji_panel(Json(req): Json<EmojiPanelReq>) -> Json<EmojiPanelResp> {
    let is_admin = crate::config::admin_user_id() == Some(req.user_id);
    let kb = crate::emoji::panel::main_panel_keyboard();
    let callbacks: Vec<String> = kb
        .inline_keyboard
        .iter()
        .flatten()
        .map(|b| b.callback_data.clone().unwrap_or_default())
        .collect();
    let start_kb = crate::bot::keyboards::start_menu_keyboard(true);
    let in_start_menu = start_kb.inline_keyboard.iter().flatten().any(|b| {
        b.callback_data
            .as_deref()
            .is_some_and(|c| c.starts_with("emoji:"))
    });
    Json(EmojiPanelResp {
        ok: true,
        is_admin,
        opens_panel: is_admin,
        rendered_text: if is_admin {
            crate::emoji::panel::main_panel_text()
        } else {
            String::new()
        },
        button_callbacks: if is_admin { callbacks } else { Vec::new() },
        in_start_menu,
    })
}
