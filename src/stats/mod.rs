mod query;
pub use query::{UserStats, DownloadStats, get_user_stats, get_download_stats, fmt_bytes};

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
