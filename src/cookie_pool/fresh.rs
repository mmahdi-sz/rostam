//! Redis-backed cookie freshness tracking, shared across dev + production.
//!
//! Two key families (deliberately NOT namespaced per environment so both
//! deployments coordinate over the same Firefox profiles):
//!   cookie:fresh:{profile}       String, TTL=COOKIE_FRESH_TTL_SECS — exists ⇒ profile cookies are fresh.
//!   cookie:refreshing:{profile}  String, NX EX lock — held while one env refreshes the profile.
//!
//! Worker logic (see app::startup::spawn_cookie_refresher): every cycle, for each
//! profile, skip if `fresh` exists; else try to take the `refreshing` lock; only
//! the lock holder opens Firefox and, on success, writes a fresh key. On failure
//! the lock is left to expire (back-off) so a broken profile is not retried every cycle.

use redis::{Client, RedisResult, aio::MultiplexedConnection};

pub struct FreshStore {
    client: Client,
}

impl FreshStore {
    pub fn new(url: &str) -> RedisResult<Self> {
        Ok(Self {
            client: Client::open(url)?,
        })
    }

    /// A fresh multiplexed connection. Failure here means Redis is unreachable.
    pub async fn conn(&self) -> RedisResult<MultiplexedConnection> {
        self.client.get_multiplexed_async_connection().await
    }
}

fn fresh_key(profile: &str) -> String {
    format!("cookie:fresh:{profile}")
}

fn lock_key(profile: &str) -> String {
    format!("cookie:refreshing:{profile}")
}

/// True if the profile's cookies are still considered fresh.
pub async fn is_fresh(conn: &mut MultiplexedConnection, profile: &str) -> RedisResult<bool> {
    let n: i64 = redis::cmd("EXISTS")
        .arg(fresh_key(profile))
        .query_async(conn)
        .await?;
    Ok(n > 0)
}

/// Mark the profile fresh for `ttl_secs`. `ts` is the refresh time (epoch secs), stored as the value.
pub async fn mark_fresh(
    conn: &mut MultiplexedConnection,
    profile: &str,
    ttl_secs: u64,
    ts: i64,
) -> RedisResult<()> {
    redis::cmd("SET")
        .arg(fresh_key(profile))
        .arg(ts)
        .arg("EX")
        .arg(ttl_secs)
        .query_async::<()>(conn)
        .await
}

/// Try to acquire the refresh lock (SET NX EX). Returns true if acquired.
pub async fn try_lock(
    conn: &mut MultiplexedConnection,
    profile: &str,
    owner: &str,
    lock_ttl_secs: u64,
) -> RedisResult<bool> {
    let res: Option<String> = redis::cmd("SET")
        .arg(lock_key(profile))
        .arg(owner)
        .arg("NX")
        .arg("EX")
        .arg(lock_ttl_secs)
        .query_async(conn)
        .await?;
    Ok(res.is_some())
}

/// Release the refresh lock (called only on a successful refresh).
pub async fn unlock(conn: &mut MultiplexedConnection, profile: &str) -> RedisResult<()> {
    let _: i64 = redis::cmd("DEL")
        .arg(lock_key(profile))
        .query_async(conn)
        .await?;
    Ok(())
}

/// Delete every refresh lock owned by `owner`. Called once at worker startup: a
/// freshly-started process cannot legitimately be mid-refresh, so any lock with
/// our owner is orphaned from a previous run that died before unlocking. Locks
/// owned by the other environment (e.g. prod) are left untouched. Returns count removed.
pub async fn clear_own_locks(conn: &mut MultiplexedConnection, owner: &str) -> RedisResult<usize> {
    let mut cursor: u64 = 0;
    let mut removed = 0;
    loop {
        let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg("cookie:refreshing:*")
            .arg("COUNT")
            .arg(100)
            .query_async(conn)
            .await?;
        for key in keys {
            let val: Option<String> = redis::cmd("GET").arg(&key).query_async(conn).await?;
            if val.as_deref() == Some(owner) {
                let _: i64 = redis::cmd("DEL").arg(&key).query_async(conn).await?;
                removed += 1;
            }
        }
        if next == 0 {
            break;
        }
        cursor = next;
    }
    Ok(removed)
}
