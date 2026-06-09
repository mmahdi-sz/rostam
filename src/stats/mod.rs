mod query;
pub use query::{UserStats, DownloadStats, get_user_stats, get_download_stats, fmt_bytes};

use std::sync::OnceLock;
use tokio_postgres::Client;

// ── global client ─────────────────────────────────────────────────────────────
// یه بار در startup ست میشه — همه ماژول‌ها بدون پاس دادن client می‌تونن ثبت کنن.
static DB: OnceLock<&'static Client> = OnceLock::new();

pub fn init(client: &'static Client) {
    let _ = DB.set(client);
}

fn db() -> Option<&'static Client> {
    DB.get().copied()
}

// ── record functions ──────────────────────────────────────────────────────────

pub async fn record_user_global(user_id: i64) {
    let Some(client) = db() else { return };
    record_user(client, user_id).await;
}

pub async fn record_user(client: &Client, user_id: i64) {
    let r = client.execute(
        "INSERT INTO stats_users (user_id, first_seen, last_seen)
         VALUES ($1, NOW(), NOW())
         ON CONFLICT (user_id) DO UPDATE SET last_seen = NOW()",
        &[&user_id],
    ).await;
    if let Err(e) = r {
        eprintln!("[stats event=record_user_failed] user_id={user_id} err={e}");
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

pub async fn record_upload_done(job_id: i64, bytes_uploaded: i64) {
    let Some(client) = db() else { return };
    let r = client.execute(
        "UPDATE stats_downloads SET upload_ok = TRUE, bytes_uploaded = $1 WHERE id = $2",
        &[&bytes_uploaded, &job_id],
    ).await;
    if let Err(e) = r {
        eprintln!("[stats event=record_upload_done_failed] job_id={job_id} err={e}");
    }
}
