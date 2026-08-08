use crate::app::dispatch::handle_update;
use crate::app::state::AppState;
use crate::bot::messaging::CAPTURED_EMOJIS;
use crate::cookie_pool::CookiePool;
use crate::emoji::FlowManager;
use crate::i18n::RESOLVED_I18N_KEYS;
use crate::log::CAPTURED_TRACES;
use crate::stats::CAPTURED_STATS;
use crate::testapi::state::clear_payloads;
use axum::Json;
use axum::response::IntoResponse;
use frankenstein::{
    client_reqwest::Bot,
    types::{CallbackQuery, Chat, ChatType, MaybeInaccessibleMessage, Message, User},
    updates::UpdateContent,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub async fn test_callback(Json(payload): Json<Value>) -> axum::response::Response {
    clear_payloads();

    let cb_data = match payload.get("callback_data") {
        Some(v) if v.is_string() => v.as_str().unwrap().to_string(),
        Some(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "callback_data must be a string",
            )
                .into_response();
        }
        None => {
            return (axum::http::StatusCode::BAD_REQUEST, "missing callback_data").into_response();
        }
    };

    let user_id = match payload.get("user_id") {
        Some(v) if v.is_i64() => v.as_i64().unwrap(),
        Some(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "user_id must be an integer",
            )
                .into_response();
        }
        None => 12345,
    };

    let username = match payload.get("username") {
        Some(v) if v.is_string() => v.as_str().unwrap().to_string(),
        Some(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "username must be a string",
            )
                .into_response();
        }
        None => "testuser".to_string(),
    };

    let api = Bot::new_url("http://127.0.0.1:14379/bot".to_string());

    let (rate_limit_tx, _) = mpsc::unbounded_channel();

    let mut state = AppState {
        api,
        cookie_pool: Arc::new(tokio::sync::Mutex::new(CookiePool::from_firefox_root(""))),
        database: None,
        flow_manager: FlowManager::new(),
        rate_limit_tx,
        user_last_update: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
    };

    let user = User::builder()
        .id(user_id as u64)
        .is_bot(false)
        .first_name("Test".to_string())
        .username(username.to_string())
        .build();

    let chat = Chat::builder()
        .id(user_id)
        .type_field(ChatType::Private)
        .build();

    let message = Message::builder()
        .message_id(100)
        .date(1600000000)
        .chat(chat)
        .build();

    let callback_query = CallbackQuery::builder()
        .id("test_cq_id".to_string())
        .from(user)
        .message(MaybeInaccessibleMessage::Message(Box::new(message)))
        .chat_instance("chat_instance".to_string())
        .data(cb_data.to_string())
        .build();

    let update = UpdateContent::CallbackQuery(Box::new(callback_query));

    let traces = Arc::new(Mutex::new(Vec::new()));
    let stats = Arc::new(Mutex::new(Vec::new()));
    let i18n_keys = Arc::new(Mutex::new(Vec::new()));
    let emojis = Arc::new(Mutex::new(Vec::new()));

    let t = traces.clone();
    let s = stats.clone();
    let i = i18n_keys.clone();
    let e = emojis.clone();

    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let _ = CAPTURED_TRACES
                .scope(t, async {
                    CAPTURED_STATS
                        .scope(s, async {
                            RESOLVED_I18N_KEYS
                                .scope(i, async {
                                    CAPTURED_EMOJIS
                                        .scope(e, async {
                                            let _ = handle_update(&mut state, update).await;
                                        })
                                        .await;
                                })
                                .await;
                        })
                        .await;
                })
                .await;
        });
        let _ = tx.send(());
    });
    let _ = rx.await;

    let payloads = crate::testapi::state::CAPTURED_PAYLOADS
        .lock()
        .unwrap()
        .clone();

    let mut message_payload = json!({});
    let mut inline_keyboard = json!([]);
    let mut reply_keyboard = json!([]);
    let mut answer_callback_query = json!({});

    for payload in &payloads {
        let method = payload
            .get("_method")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if method == "sendMessage" || method == "editMessageText" {
            message_payload = json!({
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
        } else if method == "answerCallbackQuery" {
            answer_callback_query = payload.clone();
        }
    }

    Json(json!({
        "ok": true,
        "trace": traces.lock().unwrap().clone(),
        "stats_events": stats.lock().unwrap().clone(),
        "message": message_payload,
        "inline_keyboard": inline_keyboard,
        "reply_keyboard": reply_keyboard,
        "answer_callback_query": answer_callback_query,
        "payloads": payloads,
        "errors": []
    }))
    .into_response()
}
