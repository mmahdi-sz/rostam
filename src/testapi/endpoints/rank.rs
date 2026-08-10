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
    let mut warning_message = json!({});
    let mut inline_keyboard = json!([]);
    let mut reply_keyboard = json!([]);

    if payloads.len() >= 2 {
        let first = &payloads[0];
        warning_message = json!({
            "parse_mode": first.get("parse_mode").unwrap_or(&Value::Null),
            "rendered_text": first.get("text").unwrap_or(&Value::Null),
            "inline_keyboard": first.get("reply_markup").and_then(|m| m.get("inline_keyboard")).unwrap_or(&json!([])),
        });
    }

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
        "warning_message": warning_message,
        "message": message,
        "inline_keyboard": inline_keyboard,
        "reply_keyboard": reply_keyboard,
        "errors": []
    }))
    .into_response()
}

/// Free-rank button in the /rank shop + the referral banner it opens.
/// `lang` picks the language root; `label_key` can be overridden to exercise the
/// missing-i18n-key failure path.
pub async fn test_free_rank(Json(payload): Json<Value>) -> axum::response::Response {
    let lang = payload
        .get("lang")
        .and_then(|v| v.as_str())
        .unwrap_or("fa")
        .to_string();
    let label_key = payload
        .get("label_key")
        .and_then(|v| v.as_str())
        .unwrap_or("rank.free_rank_button")
        .to_string();
    let user_id = payload
        .get("user_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(12345);

    let i18n_keys = Arc::new(Mutex::new(Vec::new()));
    let i = i18n_keys.clone();

    let (keyboard, label, banner) = crate::i18n::LANG
        .scope(lang.clone(), async move {
            RESOLVED_I18N_KEYS
                .scope(i, async move {
                    let prices = crate::rank::prices::get();
                    let kb = crate::rank::menu::build_shop_keyboard(Rank::Dalavar, &prices);
                    let label = crate::i18n::t(&label_key);
                    let banner = crate::i18n::apply_premium_to_html(&crate::i18n::tf(
                        "referral.banner",
                        &[("username", "rostam_bot"), ("user_id", &user_id.to_string())],
                    ));
                    (kb, label, banner)
                })
                .await
        })
        .await;

    let rows = serde_json::to_value(&keyboard.inline_keyboard).unwrap_or(json!([]));
    // Locate the free-rank row by its callback data.
    let free_btn = keyboard
        .inline_keyboard
        .iter()
        .flatten()
        .position(|b| b.callback_data.as_deref() == Some(crate::rank::panel::CB_REFERRAL));
    let buy_row = keyboard
        .inline_keyboard
        .iter()
        .position(|r| r.iter().any(|b| b.url.is_some()));
    let free_row = keyboard.inline_keyboard.iter().position(|r| {
        r.iter()
            .any(|b| b.callback_data.as_deref() == Some(crate::rank::panel::CB_REFERRAL))
    });
    let btn = free_row.and_then(|idx| keyboard.inline_keyboard[idx].first());

    let mut errors: Vec<String> = Vec::new();
    if free_btn.is_none() {
        errors.push("free rank button missing from shop keyboard".into());
    }
    if let (Some(b), Some(f)) = (buy_row, free_row) {
        if f != b + 1 {
            errors.push(format!("free rank row {f} is not directly below buy row {b}"));
        }
    }
    if let Some(b) = btn {
        if label.starts_with('!') {
            errors.push(format!("unresolved i18n key on label: {label}"));
        }
        if b.text.contains("tg-emoji") || b.text.contains("tg://emoji") {
            errors.push("button label carries a premium emoji tag".into());
        }
        if b.icon_custom_emoji_id.is_none() {
            errors.push("free rank button has no icon_custom_emoji_id".into());
        }
        if format!("{:?}", b.style).contains("Primary") {
        } else {
            errors.push(format!("style is not Primary: {:?}", b.style));
        }
    }
    let expandable = banner.matches("<blockquote expandable>").count();
    if expandable != 3 {
        errors.push(format!("banner has {expandable} expandable quotes, want 3"));
    }
    let want_glyphs = if lang == "fa" { ('╣', '╝') } else { ('╠', '╚') };
    if !banner.contains(want_glyphs.0) || !banner.contains(want_glyphs.1) {
        errors.push(format!("banner missing {want_glyphs:?} tree glyphs for {lang}"));
    }
    let banner_len = banner.encode_utf16().count();
    if banner_len > 4096 {
        errors.push(format!("banner is {banner_len} UTF-16 units, over the 4096 cap"));
    }

    Json(json!({
        "ok": errors.is_empty(),
        "lang": lang,
        "free_rank_button": btn.map(|b| json!({
            "label": b.text,
            "callback_data": b.callback_data,
            "style": format!("{:?}", b.style),
            "icon_custom_emoji_id": b.icon_custom_emoji_id,
        })),
        "buy_row_index": buy_row,
        "free_row_index": free_row,
        "inline_keyboard": rows,
        "banner": {
            "parse_mode": "HTML",
            "rendered_text": banner,
            "expandable_quotes": expandable,
            "custom_emoji_spans": banner.matches("<tg-emoji emoji-id=").count(),
            "utf16_len": banner_len,
        },
        "resolved_i18n_keys": i18n_keys.lock().unwrap().clone(),
        "errors": errors,
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
