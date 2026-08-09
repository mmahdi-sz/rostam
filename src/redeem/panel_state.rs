//! Code generation panel UI state stored in Redis (per-admin key).
//!
//! Key: `gencode:state:{admin_id}` -> Value: `"{rank}|{days}|{uses}"`, TTL 1 hour.

use redis::aio::MultiplexedConnection;

use crate::config;
use crate::rank::types::Rank;

const STATE_TTL_SECS: u64 = 3600;

/// Current admin selection in code generation panel.
#[derive(Debug, Clone, Copy)]
pub struct GenSelection {
    pub rank: Rank,
    pub days: i32,
    pub uses: i32,
}

impl Default for GenSelection {
    /// Default: Esfandyar, 31 days, 1 use.
    fn default() -> Self {
        Self {
            rank: Rank::Esfandyar,
            days: 31,
            uses: 1,
        }
    }
}

impl GenSelection {
    fn encode(&self) -> String {
        format!("{}|{}|{}", self.rank.as_str(), self.days, self.uses)
    }

    fn decode(s: &str) -> Option<Self> {
        let mut parts = s.split('|');
        let rank = Rank::from_str(parts.next()?)?;
        let days = parts.next()?.parse().ok()?;
        let uses = parts.next()?.parse().ok()?;
        Some(Self { rank, days, uses })
    }
}

fn key(admin_id: i64) -> String {
    format!("gencode:state:{admin_id}")
}

/// New Redis connection (per-call, low-frequency admin panel).
async fn conn() -> redis::RedisResult<MultiplexedConnection> {
    let client = redis::Client::open(config::redis_url())?;
    client.get_multiplexed_async_connection().await
}

/// Load current selection; returns default on error/missing key.
pub async fn load(admin_id: i64) -> GenSelection {
    let Ok(mut c) = conn().await else {
        return GenSelection::default();
    };
    let val: Option<String> = redis::cmd("GET")
        .arg(key(admin_id))
        .query_async(&mut c)
        .await
        .ok()
        .flatten();
    val.as_deref()
        .and_then(GenSelection::decode)
        .unwrap_or_default()
}

/// Save selection with 1-hour TTL.
pub async fn save(admin_id: i64, sel: GenSelection) {
    let Ok(mut c) = conn().await else { return };
    let _: Result<(), _> = redis::cmd("SET")
        .arg(key(admin_id))
        .arg(sel.encode())
        .arg("EX")
        .arg(STATE_TTL_SECS)
        .query_async::<()>(&mut c)
        .await;
}

/// Clear panel state (after successful code generation).
pub async fn clear(admin_id: i64) {
    let Ok(mut c) = conn().await else { return };
    let _: Result<i64, _> = redis::cmd("DEL")
        .arg(key(admin_id))
        .query_async(&mut c)
        .await;
}
