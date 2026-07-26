use axum::Json;
use serde_json::{Value, json};
use frankenstein::client_reqwest::Bot;
use std::sync::{Arc, Mutex};
use crate::rank::types::Rank;
use crate::rank::paywall::block_feature;
use crate::testapi::state::clear_payloads;
use crate::log::CAPTURED_TRACES;
use crate::stats::CAPTURED_STATS;
use crate::i18n::RESOLVED_I18N_KEYS;
use crate::bot::messaging::CAPTURED_EMOJIS;

pub async fn test_paywall(Json(payload): Json<Value>) -> Json<Value> {
    clear_payloads();
    
    let feature = payload.get("feature").and_then(|v| v.as_str()).unwrap_or("Test Feature");
    let rank_str = payload.get("rank").and_then(|v| v.as_str()).unwrap_or("Dalavar");
    
    // Parse rank manually
    let rank = match rank_str {
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

    CAPTURED_TRACES.scope(t, async {
        CAPTURED_STATS.scope(s, async {
            RESOLVED_I18N_KEYS.scope(i, async {
                CAPTURED_EMOJIS.scope(e, async {
                    block_feature(&api, chat_id, feature, rank).await;
                }).await;
            }).await;
        }).await;
    }).await;

    let payloads = crate::testapi::state::CAPTURED_PAYLOADS.lock().unwrap().clone();
    
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
}
