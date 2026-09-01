use axum::{Json, extract::Path, extract::Request};
use serde_json::Value;

pub async fn intercept_bot_request(
    Path((_token, method)): Path<(String, String)>,
    req: Request,
) -> Json<Value> {
    let content_type = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let mut payload = if content_type.starts_with("multipart/form-data") {
        let mut map = serde_json::Map::new();
        map.insert(
            "note".to_string(),
            Value::String("multipart ignored".to_string()),
        );
        let _ = axum::body::to_bytes(req.into_body(), usize::MAX).await;
        Value::Object(map)
    } else {
        let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
            .await
            .unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
    };

    if let Some(obj) = payload.as_object_mut() {
        obj.insert("_method".to_string(), Value::String(method.clone()));
    }

    crate::testapi::state::CAPTURED_PAYLOADS
        .lock()
        .unwrap()
        .push(payload.clone());

    if method == "getMe" {
        return Json(serde_json::json!({
            "ok": true,
            "result": {
                "id": 999888777,
                "is_bot": true,
                "first_name": "TestBot",
                "username": "rostam_test_bot",
                "can_join_groups": true,
                "can_read_all_group_messages": true,
                "supports_inline_queries": false,
                "can_connect_to_business": false,
                "has_main_web_app": false
            }
        }));
    }

    if method == "getChat" {
        let chat_id = payload
            .get("chat_id")
            .cloned()
            .unwrap_or(serde_json::json!(12345));
        let title = "Test Channel".to_string();
        return Json(serde_json::json!({
            "ok": true,
            "result": {
                "id": if chat_id.is_number() { chat_id } else { serde_json::json!(12345) },
                "type": "channel",
                "title": title
            }
        }));
    }

    if method == "getChatMember" {
        let uid = payload.get("user_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let is_bot = uid == 999888777;

        if is_bot {
            let bot_admin = crate::testapi::state::SIMULATED_BOT_ADMIN_STATUS
                .lock()
                .unwrap()
                .unwrap_or(true);
            if bot_admin {
                return Json(serde_json::json!({
                    "ok": true,
                    "result": {
                        "status": "administrator",
                        "user": { "id": 999888777, "is_bot": true, "first_name": "TestBot" },
                        "can_be_edited": false,
                        "is_anonymous": false,
                        "can_manage_chat": true,
                        "can_delete_messages": true,
                        "can_manage_video_chats": true,
                        "can_restrict_members": true,
                        "can_promote_members": false,
                        "can_change_info": true,
                        "can_invite_users": true,
                        "can_post_stories": true,
                        "can_edit_stories": true,
                        "can_delete_stories": true
                    }
                }));
            } else {
                return Json(serde_json::json!({
                    "ok": true,
                    "result": {
                        "status": "left",
                        "user": { "id": 999888777, "is_bot": true, "first_name": "TestBot" }
                    }
                }));
            }
        } else {
            let simulated_status = crate::testapi::state::SIMULATED_CHAT_MEMBER_STATUS
                .lock()
                .unwrap()
                .clone();
            match simulated_status.as_deref() {
                Some("error") => {
                    return Json(serde_json::json!({
                        "ok": false,
                        "error_code": 400,
                        "description": "Bad Request: user not found"
                    }));
                }
                Some("left") => {
                    return Json(serde_json::json!({
                        "ok": true,
                        "result": {
                            "status": "left",
                            "user": { "id": uid, "is_bot": false, "first_name": "TestUser" }
                        }
                    }));
                }
                Some("kicked") => {
                    return Json(serde_json::json!({
                        "ok": true,
                        "result": {
                            "status": "kicked",
                            "user": { "id": uid, "is_bot": false, "first_name": "TestUser" },
                            "until_date": 0
                        }
                    }));
                }
                Some("creator") => {
                    return Json(serde_json::json!({
                        "ok": true,
                        "result": {
                            "status": "creator",
                            "is_anonymous": false,
                            "user": { "id": uid, "is_bot": false, "first_name": "TestUser" }
                        }
                    }));
                }
                Some("administrator") => {
                    return Json(serde_json::json!({
                        "ok": true,
                        "result": {
                            "status": "administrator",
                            "user": { "id": uid, "is_bot": false, "first_name": "TestUser" },
                            "can_be_edited": false,
                            "is_anonymous": false,
                            "can_manage_chat": true,
                            "can_delete_messages": true,
                            "can_manage_video_chats": true,
                            "can_restrict_members": true,
                            "can_promote_members": false,
                            "can_change_info": true,
                            "can_invite_users": true,
                            "can_post_stories": true,
                            "can_edit_stories": true,
                            "can_delete_stories": true
                        }
                    }));
                }
                _ => {
                    return Json(serde_json::json!({
                        "ok": true,
                        "result": {
                            "status": "member",
                            "user": { "id": uid, "is_bot": false, "first_name": "TestUser" }
                        }
                    }));
                }
            }
        }
    }

    Json(serde_json::json!({
        "ok": true,
        "result": {
            "message_id": 999,
            "date": 1600000000,
            "chat": { "id": 12345, "type": "private" }
        }
    }))
}
