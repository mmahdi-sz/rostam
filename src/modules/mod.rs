pub mod cookie_refresher;

use frankenstein::client_reqwest::Bot;

/// Send an operator/admin notice to the admin PV. No-op if chat_id is 0.
pub async fn notify_admin(api: &Bot, chat_id: i64, text: &str) {
    if chat_id == 0 {
        return;
    }
    if let Err(e) = crate::bot::send_text(api, chat_id, text).await {
        eprintln!("[notify_admin event=failed] chat_id={chat_id} err={e}");
    }
}
