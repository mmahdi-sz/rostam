mod query;
pub use query::{
    get_user_stats, get_download_stats, fmt_bytes, FeatureStats, get_feature_stats, get_active_users, fmt_secs, get_action_breakdown, get_recent_errors, count_recent_errors,
};

use std::sync::OnceLock;
use tokio_postgres::Client;

use crate::rank::quota::add_traffic;

// ── global client ─────────────────────────────────────────────────────────────
static DB: OnceLock<&'static Client> = OnceLock::new();

pub fn init(client: &'static Client) {
    let _ = DB.set(client);
}

fn db() -> Option<&'static Client> {
    DB.get().copied()
}

// ── language ──────────────────────────────────────────────────────────────────

pub async fn get_user_language(user_id: i64) -> Option<String> {
    let client = db()?;
    client
        .query_opt("SELECT language FROM stats_users WHERE user_id = $1", &[&user_id])
        .await
        .ok()?
        .and_then(|row| row.get(0))
}

pub async fn set_user_language(user_id: i64, lang: &str) {
    let Some(client) = db() else { return };
    let r = client
        .execute(
            "INSERT INTO stats_users (user_id, first_seen, last_seen, language)
             VALUES ($1, NOW(), NOW(), $2)
             ON CONFLICT (user_id) DO UPDATE SET language = $2",
            &[&user_id, &lang],
        )
        .await;
    if let Err(e) = r {
        eprintln!("[stats event=set_language_failed] user_id={user_id} lang={lang} err={e}");
    }
}

// ── record functions ──────────────────────────────────────────────────────────

/// ثبت کاربر در stats. خروجی true یعنی این کاربر برای اولین بار دیده شده
/// (برای گیت زدن attribution زیرمجموعه‌گیری استفاده می‌شود).
pub async fn record_user_global(user_id: i64) -> bool {
    let Some(client) = db() else { return false };
    record_user(client, user_id).await
}

// ── generic feature event ───────────────────────────────────────────────────────
// هر فیچر (stt/denoise/upscale/separation/gwm/asr/...) بدون تغییر امضاش با این تابع
// آمار ثبت می‌کنه. amount = ثانیه‌ی صدا یا تعداد، بسته به فیچر.
pub async fn record_event_global(feature: &str, action: &str, status: &str, amount: i64) {
    let Some(client) = db() else { return };
    let r = client.execute(
        "INSERT INTO stats_events (user_id, feature, action, status, amount)
         VALUES ($1, $2, $3, $4, $5)",
        &[&0i64, &feature, &action, &status, &amount],
    ).await;
    if let Err(e) = r {
        eprintln!("[stats event=record_event_failed] feature={feature} action={action} err={e}");
    }
}

// همون record_event_global ولی با user_id مشخص (وقتی در دسترسه).
pub async fn record_event_user(user_id: i64, feature: &str, action: &str, status: &str, amount: i64) {
    let Some(client) = db() else { return };
    let r = client.execute(
        "INSERT INTO stats_events (user_id, feature, action, status, amount)
         VALUES ($1, $2, $3, $4, $5)",
        &[&user_id, &feature, &action, &status, &amount],
    ).await;
    if let Err(e) = r {
        eprintln!("[stats event=record_event_failed] feature={feature} action={action} err={e}");
    }
}

// ── error log ───────────────────────────────────────────────────────────────────
// خطاهای مهم فیچرها برای دکمه «خطاهای ۱ روز گذشته».
pub async fn record_error_global(feature: &str, message: &str) {
    let Some(client) = db() else { return };
    // پیام رو کوتاه نگه می‌داریم که جدول و پیام تلگرام منفجر نشه.
    let trimmed: String = message.chars().take(500).collect();
    let r = client.execute(
        "INSERT INTO stats_errors (feature, message) VALUES ($1, $2)",
        &[&feature, &trimmed],
    ).await;
    if let Err(e) = r {
        eprintln!("[stats event=record_error_failed] feature={feature} err={e}");
    }
}

/// خروجی true یعنی ردیف تازه insert شد (کاربر قبلاً وجود نداشت).
pub async fn record_user(client: &Client, user_id: i64) -> bool {
    let r = client.query_one(
        "INSERT INTO stats_users (user_id, first_seen, last_seen)
         VALUES ($1, NOW(), NOW())
         ON CONFLICT (user_id) DO UPDATE SET last_seen = NOW()
         RETURNING (xmax = 0) AS inserted",
        &[&user_id],
    ).await;
    match r {
        Ok(row) => row.get(0),
        Err(e) => {
            eprintln!("[stats event=record_user_failed] user_id={user_id} err={e}");
            false
        }
    }
}

pub async fn record_download_start(user_id: i64) -> Option<i64> {
    let client = db()?;
    let row = client.query_opt(
        "INSERT INTO stats_downloads (user_id) VALUES ($1) RETURNING id",
        &[&user_id],
    ).await;
    match row {
        Ok(Some(r)) => Some(r.get(0)),
        Ok(None) => None,
        Err(e) => {
            eprintln!("[stats event=record_download_start_failed] user_id={user_id} err={e}");
            None
        }
    }
}

pub async fn record_download_done(job_id: i64, bytes_downloaded: i64) {
    let Some(client) = db() else { return };
    let r = client.execute(
        "UPDATE stats_downloads SET bytes_downloaded = $1 WHERE id = $2",
        &[&bytes_downloaded, &job_id],
    ).await;
    if let Err(e) = r {
        eprintln!("[stats event=record_download_done_failed] job_id={job_id} err={e}");
    }
}

pub async fn record_upload_done(job_id: i64, user_id: i64, bytes_uploaded: i64) {
    let Some(client) = db() else { return };

    let r = client.execute(
        "UPDATE stats_downloads SET upload_ok = TRUE, bytes_uploaded = $1 WHERE id = $2",
        &[&bytes_uploaded, &job_id],
    ).await;
    if let Err(e) = r {
        eprintln!("[stats event=record_upload_done_failed] job_id={job_id} err={e}");
        return;
    }

    // first_upload_at رو ست کن اگه هنوز نداره — و مقدار رو بگیر
    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let first_upload_at = match client.query_opt(
        "UPDATE stats_users
         SET first_upload_at = COALESCE(first_upload_at, $2)
         WHERE user_id = $1
         RETURNING first_upload_at",
        &[&user_id, &now_epoch],
    ).await {
        Ok(Some(row)) => row.get::<_, Option<i64>>(0).unwrap_or(now_epoch),
        _ => now_epoch,
    };

    if let Err(e) = add_traffic(client, user_id, bytes_uploaded, first_upload_at).await {
        eprintln!("[stats event=add_traffic_failed] user_id={user_id} err={e}");
    }
}
