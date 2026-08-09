use serde_json::Value;
use std::sync::Mutex;

pub static CAPTURED_PAYLOADS: Mutex<Vec<Value>> = Mutex::new(Vec::new());

pub fn clear_payloads() {
    CAPTURED_PAYLOADS.lock().unwrap().clear();
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
