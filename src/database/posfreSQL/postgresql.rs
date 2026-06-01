use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio_postgres::{Client, NoTls};

use crate::cookie_pool::{CookiePoolSnapshot, CookieSource, CooldownEntry};

pub struct PostgresDatabase {
    client: Client,
}

impl PostgresDatabase {
    pub async fn connect(database_url: &str) -> Result<Self, tokio_postgres::Error> {
        let (client, connection) = tokio_postgres::connect(database_url, NoTls).await?;

        tokio::spawn(async move {
            if let Err(error) = connection.await {
                eprintln!("postgres connection failed: {error}");
            }
        });

        let database = Self { client };
        database.init_schema().await?;
        Ok(database)
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub async fn save_snapshot(
        &self,
        snapshot: &CookiePoolSnapshot,
    ) -> Result<(), tokio_postgres::Error> {
        self.save_available_cookies(&snapshot.available_cookies).await?;
        self.save_last_used(snapshot.last_used_cookie.as_deref())
            .await?;
        self.save_cooldowns(&snapshot.cooldown_list).await?;
        Ok(())
    }

    pub async fn load_state(
        &self,
    ) -> Result<(Option<String>, Vec<CooldownEntry>), tokio_postgres::Error> {
        self.cleanup_expired_cooldowns().await?;

        let last_used_cookie = self
            .client
            .query_opt("SELECT last_used_cookie FROM cookie_pool_state WHERE id = TRUE", &[])
            .await?
            .and_then(|row| row.get::<_, Option<String>>(0));

        let cooldown_rows = self
            .client
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
        &self,
        cookie_id: Option<&str>,
    ) -> Result<(), tokio_postgres::Error> {
        self.client
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
        &self,
        entry: &CooldownEntry,
    ) -> Result<(), tokio_postgres::Error> {
        self.client
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

    async fn init_schema(&self) -> Result<(), tokio_postgres::Error> {
        self.client
            .batch_execute(include_str!("schema.sql"))
            .await?;

        Ok(())
    }

    async fn save_available_cookies(
        &self,
        cookies: &[CookieSource],
    ) -> Result<(), tokio_postgres::Error> {
        for cookie in cookies {
            self.client
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

    async fn save_cooldowns(
        &self,
        cooldowns: &[CooldownEntry],
    ) -> Result<(), tokio_postgres::Error> {
        self.cleanup_expired_cooldowns().await?;

        for cooldown in cooldowns {
            self.save_cooldown(cooldown).await?;
        }

        Ok(())
    }

    async fn cleanup_expired_cooldowns(&self) -> Result<(), tokio_postgres::Error> {
        self.client
            .execute(
                "DELETE FROM cookie_pool_cooldowns WHERE expire_at_epoch <= $1",
                &[&now_epoch()],
            )
            .await?;

        Ok(())
    }
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
