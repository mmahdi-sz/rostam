use crate::bot::messaging::CAPTURED_EMOJIS;
use crate::i18n::RESOLVED_I18N_KEYS;
use crate::log::CAPTURED_TRACES;
use crate::rank::paywall::block_feature;
use crate::rank::types::Rank;
use crate::stats::CAPTURED_STATS;
use crate::testapi::state::clear_payloads;
use axum::Json;
use axum::response::IntoResponse;
use frankenstein::client_reqwest::Bot;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub async fn test_paywall(Json(payload): Json<Value>) -> axum::response::Response {
    clear_payloads();

    let feature = match payload.get("feature") {
        Some(v) if v.is_string() => v.as_str().unwrap().to_string(),
        Some(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "feature must be a string",
            )
                .into_response();
        }
        None => "Test Feature".to_string(),
    };

    let rank_str = match payload.get("rank") {
        Some(v) if v.is_string() => v.as_str().unwrap().to_string(),
        Some(_) => {
            return (axum::http::StatusCode::BAD_REQUEST, "rank must be a string").into_response();
        }
        None => "Dalavar".to_string(),
    };

    // Parse rank manually
    let rank = match rank_str.as_str() {
        "Sepahbod" => Rank::Sepahbod,
        "Esfandyar" => Rank::Esfandyar,
        "Sohrab" => Rank::Sohrab,
        "Rostam" => Rank::Rostam,
        _ => Rank::Dalavar,
    };

    let chat_id = 12345;
    let api = Bot::new_url("http://127.0.0.1:14379/bot".to_string());

    let traces = Arc::new(Mutex::new(Vec::new()));
    let stats = Arc::new(Mutex::new(Vec::new()));
    let i18n_keys = Arc::new(Mutex::new(Vec::new()));
    let emojis = Arc::new(Mutex::new(Vec::new()));

    let t = traces.clone();
    let s = stats.clone();
    let i = i18n_keys.clone();
    let e = emojis.clone();

    CAPTURED_TRACES
        .scope(t, async {
            CAPTURED_STATS
                .scope(s, async {
                    RESOLVED_I18N_KEYS
                        .scope(i, async {
                            CAPTURED_EMOJIS
                                .scope(e, async {
                                    block_feature(&api, chat_id, &feature, rank).await;
                                })
                                .await;
                        })
                        .await;
                })
                .await;
        })
        .await;

    let payloads = crate::testapi::state::CAPTURED_PAYLOADS
        .lock()
        .unwrap()
        .clone();

    // Process payloads to match the requested output format
    let mut message = json!({});
    let mut inline_keyboard = json!([]);
    let mut reply_keyboard = json!([]);

    if let Some(payload) = payloads.last() {
        message = json!({
            "parse_mode": payload.get("parse_mode").unwrap_or(&Value::Null),
            "rendered_text": payload.get("text").unwrap_or(&Value::Null),
            "resolved_i18n_keys": i18n_keys.lock().unwrap().clone(),
            "custom_emoji_spans": emojis.lock().unwrap().clone(),
        });

        if let Some(markup) = payload.get("reply_markup") {
            if let Some(ik) = markup.get("inline_keyboard") {
                inline_keyboard = ik.clone();
            }
            if let Some(rk) = markup.get("keyboard") {
                reply_keyboard = rk.clone();
            }
        }
    }

    Json(json!({
        "ok": true,
        "trace": traces.lock().unwrap().clone(),
        "stats_events": stats.lock().unwrap().clone(),
        "message": message,
        "inline_keyboard": inline_keyboard,
        "reply_keyboard": reply_keyboard,
        "errors": []
    }))
    .into_response()
}

pub async fn test_rank_panel(Json(payload): Json<Value>) -> axum::response::Response {
    let user_id = payload
        .get("user_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(12345);
    Json(json!({
        "ok": true,
        "user_id": user_id,
        "rank": "Dalavar",
        "days_remaining": 0,
        "panel_title": crate::i18n::t("rank.panel_title"),
    }))
    .into_response()
}
