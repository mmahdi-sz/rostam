use axum::{extract::Path, Json};
use serde_json::Value;

pub async fn intercept_bot_request(
    Path((_token, method)): Path<(String, String)>,
    Json(mut payload): Json<Value>,
) -> Json<Value> {
    // Inject the method name into the payload so tests can assert what was called
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("_method".to_string(), Value::String(method.clone()));
    }

    crate::testapi::state::CAPTURED_PAYLOADS.lock().unwrap().push(payload);
    
    // Return a dummy successful Telegram API response
    Json(serde_json::json!({
        "ok": true,
        "result": {
            "message_id": 999,
            "date": 1600000000,
            "chat": { "id": 12345, "type": "private" }
        }
    }))
}
