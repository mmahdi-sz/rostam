use std::time::Duration;

use deadpool_postgres::{
    Config, ManagerConfig, Object, Pool, PoolConfig, RecyclingMethod, Runtime, Timeouts,
};
use tokio_postgres::NoTls;

use crate::config;
use crate::cookie_pool::{CookiePoolSnapshot, CooldownEntry};

pub mod cookie_pool;

#[derive(Clone)]
pub struct PostgresDatabase {
    pool: Pool,
}

impl PostgresDatabase {
    pub async fn connect(database_url: &str) -> anyhow::Result<Self> {
        let pool = create_pool(database_url)?;

        // Run Refinery migrations on startup using a temporary pooled connection
        {
            let mut client = pool.get().await.map_err(|e| {
                anyhow::anyhow!("failed to checkout migration connection from pool: {e}")
            })?;
            Self::init_schema(&mut *client).await?;
        }

        Ok(Self { pool })
    }

    pub async fn get(&self) -> Result<Object, deadpool_postgres::PoolError> {
        self.pool.get().await
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    pub async fn save_snapshot(&self, snapshot: &CookiePoolSnapshot) -> anyhow::Result<()> {
        let client = self
            .get()
            .await
            .map_err(|e| anyhow::anyhow!("failed to get db client: {e}"))?;
        cookie_pool::save_snapshot(&client, snapshot)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    pub async fn load_state(&self) -> anyhow::Result<(Option<String>, Vec<CooldownEntry>)> {
        let client = self
            .get()
            .await
            .map_err(|e| anyhow::anyhow!("failed to get db client: {e}"))?;
        cookie_pool::load_state(&client)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    #[allow(dead_code)]
    pub async fn save_last_used(&self, cookie_id: Option<&str>) -> anyhow::Result<()> {
        let client = self
            .get()
            .await
            .map_err(|e| anyhow::anyhow!("failed to get db client: {e}"))?;
        cookie_pool::save_last_used(&client, cookie_id)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    #[allow(dead_code)]
    pub async fn save_cooldown(&self, entry: &CooldownEntry) -> anyhow::Result<()> {
        let client = self
            .get()
            .await
            .map_err(|e| anyhow::anyhow!("failed to get db client: {e}"))?;
        cookie_pool::save_cooldown(&client, entry)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    async fn init_schema(client: &mut tokio_postgres::Client) -> anyhow::Result<()> {
        mod embedded {
            use refinery::embed_migrations;
            embed_migrations!("migrations");
        }

        match embedded::migrations::runner().run_async(client).await {
            Ok(report) => {
                tracing::info!(
                    event = "refinery_migrations_applied",
                    applied = report.applied_migrations().len()
                );
            }
            Err(e) => {
                eprintln!("[db event=refinery_migration_failed] err={e}");
                return Err(anyhow::anyhow!("refinery migration failed: {e}"));
            }
        }

        Ok(())
    }
}

fn create_pool(database_url: &str) -> anyhow::Result<Pool> {
    let mut cfg = Config::new();
    cfg.url = Some(database_url.to_string());
    cfg.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    let mut pool_cfg = PoolConfig::new(config::database_pool_size());
    pool_cfg.timeouts = Timeouts {
        wait: Some(Duration::from_millis(2000)),
        create: Some(Duration::from_millis(3000)),
        recycle: Some(Duration::from_millis(1000)),
    };
    cfg.pool = Some(pool_cfg);

    let pool = cfg.create_pool(Some(Runtime::Tokio1), NoTls)?;
    Ok(pool)
}
