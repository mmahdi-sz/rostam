//! Statistics, database metric logging, user tracking, and error reporting.

mod query;
#[allow(unused_imports)]
pub use query::{
    FeatureStats, Periods, count_recent_errors, fmt_bytes, fmt_secs, get_action_breakdown,
    get_active_users, get_download_stats, get_feature_stats, get_recent_errors, get_user_stats,
};

use redis::aio::MultiplexedConnection;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock as StdRwLock};
use tokio::sync::{OnceCell, RwLock as TokioRwLock};
use tokio_postgres::Client;

use crate::config;
use crate::rank::quota::add_traffic;

// ── global client & URL ──────────────────────────────────────────────────────
static DB: OnceLock<&'static Client> = OnceLock::new();
static DB_URL: OnceLock<String> = OnceLock::new();
static DB_CLIENT: OnceLock<Arc<TokioRwLock<Option<Arc<Client>>>>> = OnceLock::new();
static LANG_CACHE: OnceLock<StdRwLock<HashMap<i64, String>>> = OnceLock::new();
static REDIS_CONN: OnceCell<MultiplexedConnection> = OnceCell::const_new();

fn lang_cache() -> &'static StdRwLock<HashMap<i64, String>> {
    LANG_CACHE.get_or_init(|| StdRwLock::new(HashMap::new()))
}

async fn redis_conn() -> Option<MultiplexedConnection> {
    REDIS_CONN
        .get_or_try_init(|| async {
            let client = redis::Client::open(config::redis_url())?;
            client.get_multiplexed_async_connection().await
        })
        .await
        .ok()
        .cloned()
}

pub fn init(client: &'static Client) {
    let _ = DB.set(client);
}

pub async fn get_db_client() -> Option<Arc<Client>> {
    let url = DB_URL.get().cloned().or_else(config::database_url)?;
    let lock = DB_CLIENT.get_or_init(|| Arc::new(TokioRwLock::new(None)));

    {
        let guard = lock.read().await;
        if let Some(ref client) = *guard {
            if !client.is_closed() {
                return Some(client.clone());
            }
        }
    }

    let mut guard = lock.write().await;
    if let Some(ref client) = *guard {
        if !client.is_closed() {
            return Some(client.clone());
        }
    }

    match tokio_postgres::connect(&url, tokio_postgres::NoTls).await {
        Ok((client, conn)) => {
            tokio::spawn(async move {
                if let Err(e) = conn.await {
                    eprintln!("[postgres event=connection_closed] {e}");
                }
            });
            let client_arc = Arc::new(client);
            *guard = Some(client_arc.clone());
            println!("[postgres event=reconnected_successfully]");
            Some(client_arc)
        }
        Err(e) => {
            eprintln!("[postgres event=reconnect_failed] err={e}");
            None
        }
    }
}

fn db() -> Option<&'static Client> {
    DB.get().copied().filter(|c| !c.is_closed())
}

// ── language ──────────────────────────────────────────────────────────────────

pub async fn get_user_language(user_id: i64) -> Option<String> {
    // 1. RAM Cache (0 ms)
    if let Ok(guard) = lang_cache().read() {
        if let Some(lang) = guard.get(&user_id) {
            return Some(lang.clone());
        }
    }

    // 2. Redis Persistent Cache
    if let Some(mut c) = redis_conn().await {
        let key = format!("user:lang:{user_id}");
        let res: Option<String> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut c)
            .await
            .ok()
            .flatten();
        if let Some(ref l) = res {
            if let Ok(mut guard) = lang_cache().write() {
                guard.insert(user_id, l.clone());
            }
            return Some(l.clone());
        }
    }

    // 3. PostgreSQL Database
    let client = get_db_client().await?;
    let lang: Option<String> = client
        .query_opt(
            "SELECT language FROM stats_users WHERE user_id = $1",
            &[&user_id],
        )
        .await
        .ok()?
        .and_then(|row| row.get(0));

    if let Some(ref l) = lang {
        if let Ok(mut guard) = lang_cache().write() {
            guard.insert(user_id, l.clone());
        }
        if let Some(mut c) = redis_conn().await {
            let key = format!("user:lang:{user_id}");
            let _: Result<(), _> = redis::cmd("SET").arg(&key).arg(l).query_async(&mut c).await;
        }
    }
    lang
}

pub async fn set_user_language(user_id: i64, lang: &str) {
    // 1. RAM Cache
    if let Ok(mut guard) = lang_cache().write() {
        guard.insert(user_id, lang.to_string());
    }

    // 2. Redis Persistent Cache
    if let Some(mut c) = redis_conn().await {
        let key = format!("user:lang:{user_id}");
        let _: Result<(), _> = redis::cmd("SET")
            .arg(&key)
            .arg(lang)
            .query_async(&mut c)
            .await;
    }

    // 3. PostgreSQL Database
    let Some(client) = get_db_client().await else {
        return;
    };
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

#[cfg(feature = "testapi")]
tokio::task_local! {
    pub static CAPTURED_STATS: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>;
}

// ── generic feature event ───────────────────────────────────────────────────────
// هر فیچر (stt/denoise/upscale/separation/gwm/asr/...) بدون تغییر امضاش با این تابع
// آمار ثبت می‌کنه. amount = ثانیه‌ی صدا یا تعداد، بسته به فیچر.
pub async fn record_event_global(feature: &str, action: &str, status: &str, amount: i64) {
    crate::metrics::get()
        .requests_total
        .with_label_values(&[feature, status])
        .inc();

    #[cfg(feature = "testapi")]
    let _ = CAPTURED_STATS.try_with(|arc| {
        if let Ok(mut lock) = arc.lock() {
            lock.push(serde_json::json!({
                "user_id": 0,
                "feature": feature,
                "action": action,
                "status": status,
                "amount": amount,
            }));
        }
    });

    let Some(client) = db() else { return };
    let r = client
        .execute(
            "INSERT INTO stats_events (user_id, feature, action, status, amount)
         VALUES ($1, $2, $3, $4, $5)",
            &[&0i64, &feature, &action, &status, &amount],
        )
        .await;
    if let Err(e) = r {
        eprintln!("[stats event=record_event_failed] feature={feature} action={action} err={e}");
    }
}

// همون record_event_global ولی با user_id مشخص (وقتی در دسترسه).
pub async fn record_event_user(
    user_id: i64,
    feature: &str,
    action: &str,
    status: &str,
    amount: i64,
) {
    crate::metrics::get()
        .requests_total
        .with_label_values(&[feature, status])
        .inc();

    #[cfg(feature = "testapi")]
    let _ = CAPTURED_STATS.try_with(|arc| {
        if let Ok(mut lock) = arc.lock() {
            lock.push(serde_json::json!({
                "user_id": user_id,
                "feature": feature,
                "action": action,
                "status": status,
                "amount": amount,
            }));
        }
    });

    let Some(client) = db() else { return };
    let r = client
        .execute(
            "INSERT INTO stats_events (user_id, feature, action, status, amount)
         VALUES ($1, $2, $3, $4, $5)",
            &[&user_id, &feature, &action, &status, &amount],
        )
        .await;
    if let Err(e) = r {
        eprintln!("[stats event=record_event_failed] feature={feature} action={action} err={e}");
    }
}

// ── error log ───────────────────────────────────────────────────────────────────
// خطاهای مهم فیچرها برای دکمه «خطاهای ۱ روز گذشته».
pub async fn record_error_global(feature: &str, message: impl std::fmt::Display) {
    crate::metrics::get()
        .errors_total
        .with_label_values(&[feature])
        .inc();

    let Some(client) = db() else { return };
    // پیام رو کوتاه نگه می‌داریم که جدول و پیام تلگرام منفجر نشه.
    let msg_str = message.to_string();
    let trimmed: String = msg_str.chars().take(500).collect();
    let r = client
        .execute(
            "INSERT INTO stats_errors (feature, message) VALUES ($1, $2)",
            &[&feature, &trimmed],
        )
        .await;
    if let Err(e) = r {
        eprintln!("[stats event=record_error_failed] feature={feature} err={e}");
    }
}

/// خروجی true یعنی ردیف تازه insert شد (کاربر قبلاً وجود نداشت).
pub async fn record_user(client: &Client, user_id: i64) -> bool {
    let r = client
        .query_one(
            "INSERT INTO stats_users (user_id, first_seen, last_seen)
         VALUES ($1, NOW(), NOW())
         ON CONFLICT (user_id) DO UPDATE SET last_seen = NOW()
         RETURNING (xmax = 0) AS inserted",
            &[&user_id],
        )
        .await;
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
    let row = client
        .query_opt(
            "INSERT INTO stats_downloads (user_id) VALUES ($1) RETURNING id",
            &[&user_id],
        )
        .await;
    match row {
        Ok(Some(r)) => Some(r.get(0)),
        Ok(None) => None,
        Err(e) => {
            eprintln!("[stats event=record_download_start_failed] user_id={user_id} err={e}");
            None
        }
    }
}

pub async fn record_download_done(
    job_id: i64,
    bytes_downloaded: i64,
    duration: Option<i32>,
    bitrate: Option<i64>,
) {
    let Some(client) = db() else { return };
    let r = client.execute(
        "UPDATE stats_downloads SET bytes_downloaded = $1, duration = $2, bitrate = $3 WHERE id = $4",
        &[&bytes_downloaded, &duration, &bitrate, &job_id],
    ).await;
    if let Err(e) = r {
        eprintln!("[stats event=record_download_done_failed] job_id={job_id} err={e}");
    }
}

pub async fn record_upload_done(job_id: i64, user_id: i64, bytes_uploaded: i64) {
    let Some(client) = db() else { return };

    let r = client
        .execute(
            "UPDATE stats_downloads SET upload_ok = TRUE, bytes_uploaded = $1 WHERE id = $2",
            &[&bytes_uploaded, &job_id],
        )
        .await;
    if let Err(e) = r {
        eprintln!("[stats event=record_upload_done_failed] job_id={job_id} err={e}");
        return;
    }

    // first_upload_at رو ست کن اگه هنوز نداره — و مقدار رو بگیر
    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let first_upload_at = match client
        .query_opt(
            "UPDATE stats_users
         SET first_upload_at = COALESCE(first_upload_at, $2)
         WHERE user_id = $1
         RETURNING first_upload_at",
            &[&user_id, &now_epoch],
        )
        .await
    {
        Ok(Some(row)) => row.get::<_, Option<i64>>(0).unwrap_or(now_epoch),
        _ => now_epoch,
    };

    if let Err(e) = add_traffic(client, user_id, bytes_uploaded, first_upload_at).await {
        eprintln!("[stats event=add_traffic_failed] user_id={user_id} err={e}");
    }
}
