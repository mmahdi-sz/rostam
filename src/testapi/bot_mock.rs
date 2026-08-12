use axum::{Json, extract::Path, extract::Request};
use serde_json::Value;

pub async fn intercept_bot_request(
    Path((_token, method)): Path<(String, String)>,
    req: Request,
) -> Json<Value> {
    let content_type = req.headers().get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    
    let mut payload = if content_type.starts_with("multipart/form-data") {
        let mut map = serde_json::Map::new();
        map.insert("note".to_string(), Value::String("multipart ignored".to_string()));
        let _ = axum::body::to_bytes(req.into_body(), usize::MAX).await;
        Value::Object(map)
    } else {
        let bytes = axum::body::to_bytes(req.into_body(), usize::MAX).await.unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
    };

    if let Some(obj) = payload.as_object_mut() {
        obj.insert("_method".to_string(), Value::String(method.clone()));
    }

    crate::testapi::state::CAPTURED_PAYLOADS
        .lock()
        .unwrap()
        .push(payload);

    Json(serde_json::json!({
        "ok": true,
        "result": {
            "message_id": 999,
            "date": 1600000000,
            "chat": { "id": 12345, "type": "private" }
        }
    }))
}
