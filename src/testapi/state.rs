use serde_json::Value;
use std::sync::Mutex;

pub static CAPTURED_PAYLOADS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
pub static SIMULATED_CHAT_MEMBER_STATUS: Mutex<Option<String>> = Mutex::new(None);
pub static SIMULATED_BOT_ADMIN_STATUS: Mutex<Option<bool>> = Mutex::new(None);

pub fn clear_payloads() {
    CAPTURED_PAYLOADS.lock().unwrap().clear();
}

pub fn set_simulated_chat_member(status: Option<&str>) {
    *SIMULATED_CHAT_MEMBER_STATUS.lock().unwrap() = status.map(|s| s.to_string());
}

pub fn set_simulated_bot_admin(is_admin: Option<bool>) {
    *SIMULATED_BOT_ADMIN_STATUS.lock().unwrap() = is_admin;
}

/// Database connection for endpoints requiring DB access. Returns `None` if unreachable.
pub async fn db() -> &'static Option<crate::database::postgresql::PostgresDatabase> {
    static DB: tokio::sync::OnceCell<Option<crate::database::postgresql::PostgresDatabase>> =
        tokio::sync::OnceCell::const_new();
    DB.get_or_init(|| async {
        let url = crate::config::database_url()?;
        match crate::database::postgresql::PostgresDatabase::connect(&url).await {
            Ok(db) => Some(db),
            Err(e) => {
                eprintln!("[testapi] db connect failed: {e}");
                None
            }
        }
    })
    .await
}
