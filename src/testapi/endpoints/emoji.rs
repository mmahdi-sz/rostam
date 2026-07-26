use axum::Json;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use crate::bot::messaging::expand_and_entify_for_test;
use crate::bot::messaging::CAPTURED_EMOJIS;
use crate::log::CAPTURED_TRACES;

pub async fn test_premium_render(Json(payload): Json<Value>) -> Json<Value> {
    let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let chat_id = payload.get("chat_id").and_then(|v| v.as_i64()).unwrap_or(12345);

    let emojis = Arc::new(Mutex::new(Vec::new()));
    let traces = Arc::new(Mutex::new(Vec::new()));

    let e = emojis.clone();
    let t = traces.clone();

    let (rendered, entities, _) = CAPTURED_TRACES.scope(t, async {
        CAPTURED_EMOJIS.scope(e, async {
            expand_and_entify_for_test(text, chat_id).await
        }).await
    }).await;

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
}
