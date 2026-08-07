use serde_json::Value;
use std::sync::Mutex;

pub static CAPTURED_PAYLOADS: Mutex<Vec<Value>> = Mutex::new(Vec::new());

pub fn clear_payloads() {
    CAPTURED_PAYLOADS.lock().unwrap().clear();
}

/// اتصال دیتابیس برای اندپوینت‌هایی که مسیر واقعی‌شان به DB می‌خورد (redeem و
/// خرج امتیاز زیرمجموعه‌گیری). همان `PostgresDatabase::connect` مسیر
/// production است، یک‌بار برای کل پروسه‌ی تست.
///
/// `None` یعنی وصل نشد؛ اندپوینت آن را عیناً به هندلر می‌دهد، پس شاخه‌ی
/// «دیتابیس نداریم» هم از مسیر واقعی تست می‌شود، نه با mock.
///
/// ponytail: بدون pool و بدون retry — تست‌ها روی همین یک کلاینت مشترک اجرا
/// می‌شوند، دقیقاً مثل خود بات.
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
