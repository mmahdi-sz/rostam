use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_postgres::Client;

use crate::cookie_pool::{CookiePoolSnapshot, CookieSource, CooldownEntry};

pub async fn save_snapshot(
    client: &Client,
    snapshot: &CookiePoolSnapshot,
) -> Result<(), tokio_postgres::Error> {
    save_available_cookies(client, &snapshot.available_cookies).await?;
    save_last_used(client, snapshot.last_used_cookie.as_deref()).await?;
    save_cooldowns(client, &snapshot.cooldown_list).await?;
    Ok(())
}

pub async fn load_state(
    client: &Client,
) -> Result<(Option<String>, Vec<CooldownEntry>), tokio_postgres::Error> {
    cleanup_expired_cooldowns(client).await?;

    let last_used_cookie = client
        .query_opt(
            "SELECT last_used_cookie FROM cookie_pool_state WHERE id = TRUE",
            &[],
        )
        .await?
        .and_then(|row| row.get::<_, Option<String>>(0));

    let cooldown_rows = client
        .query(
            "SELECT cookie_id, expire_at_epoch FROM cookie_pool_cooldowns ORDER BY expire_at_epoch ASC LIMIT 20",
            &[],
        )
        .await?;

    let cooldowns = cooldown_rows
        .into_iter()
        .filter_map(|row| {
            let cookie_id = row.get::<_, String>(0);
            let expire_at_epoch = row.get::<_, i64>(1);
            let expire_at = system_time_from_epoch(expire_at_epoch)?;

            Some(CooldownEntry {
                cookie_id,
                expire_at,
            })
        })
        .collect();

    Ok((last_used_cookie, cooldowns))
}

pub async fn save_last_used(
    client: &Client,
    cookie_id: Option<&str>,
) -> Result<(), tokio_postgres::Error> {
    client
        .execute(
            "INSERT INTO cookie_pool_state (id, last_used_cookie, updated_at_epoch)
             VALUES (TRUE, $1, $2)
             ON CONFLICT (id) DO UPDATE SET
                last_used_cookie = EXCLUDED.last_used_cookie,
                updated_at_epoch = EXCLUDED.updated_at_epoch",
            &[&cookie_id, &now_epoch()],
        )
        .await?;

    Ok(())
}

pub async fn save_cooldown(
    client: &Client,
    entry: &CooldownEntry,
) -> Result<(), tokio_postgres::Error> {
    client
        .execute(
            "INSERT INTO cookie_pool_cooldowns (cookie_id, expire_at_epoch)
             VALUES ($1, $2)
             ON CONFLICT (cookie_id) DO UPDATE SET
                expire_at_epoch = EXCLUDED.expire_at_epoch",
            &[&entry.cookie_id, &epoch_from_system_time(entry.expire_at)],
        )
        .await?;

    Ok(())
}

pub async fn save_available_cookies(
    client: &Client,
    cookies: &[CookieSource],
) -> Result<(), tokio_postgres::Error> {
    for cookie in cookies {
        client
            .execute(
                "INSERT INTO cookie_pool_cookies
                    (cookie_id, profile_name, profile_dir, cookies_file, updated_at_epoch)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (cookie_id) DO UPDATE SET
                    profile_name = EXCLUDED.profile_name,
                    profile_dir = EXCLUDED.profile_dir,
                    cookies_file = EXCLUDED.cookies_file,
                    updated_at_epoch = EXCLUDED.updated_at_epoch",
                &[
                    &cookie.id,
                    &cookie.profile_name,
                    &path_to_string(&cookie.profile_dir),
                    &path_to_string(&cookie.cookies_sqlite),
                    &now_epoch(),
                ],
            )
            .await?;
    }

    Ok(())
}

pub async fn save_cooldowns(
    client: &Client,
    cooldowns: &[CooldownEntry],
) -> Result<(), tokio_postgres::Error> {
    cleanup_expired_cooldowns(client).await?;

    for cooldown in cooldowns {
        save_cooldown(client, cooldown).await?;
    }

    Ok(())
}

pub async fn cleanup_expired_cooldowns(client: &Client) -> Result<(), tokio_postgres::Error> {
    client
        .execute(
            "DELETE FROM cookie_pool_cooldowns WHERE expire_at_epoch <= $1",
            &[&now_epoch()],
        )
        .await?;

    Ok(())
}

fn path_to_string(path: &PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

fn now_epoch() -> i64 {
    epoch_from_system_time(SystemTime::now())
}

fn epoch_from_system_time(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn system_time_from_epoch(epoch: i64) -> Option<SystemTime> {
    let epoch = u64::try_from(epoch).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(epoch))
}
