use redis::aio::MultiplexedConnection;
use tokio::sync::OnceCell;

use crate::config;

static REDIS_CLIENT: OnceCell<redis::Client> = OnceCell::const_new();

pub(crate) async fn conn() -> redis::RedisResult<MultiplexedConnection> {
    let client = REDIS_CLIENT
        .get_or_try_init(|| async { redis::Client::open(config::redis_url()) })
        .await?;
    client.get_multiplexed_async_connection().await
}

pub(crate) const JOINED_TTL_SECS: u64 = 86400 * 30;
pub(crate) const NOT_JOINED_TTL_SECS: u64 = 300;
pub(crate) const ENABLED_KEY: &str = "force_join:enabled";
pub(crate) const NEXT_ID_KEY: &str = "force_join:next_id";
pub(crate) const LOCK_IDS_KEY: &str = "force_join:lock_ids";

// Phase 4 Coverage Note: Redis key formatting helpers below (`lock_hash_key`, `joined_key`,
// `counted_key`, `already_count_key`, `linked_count_key`) are deterministic string formatters
// intentionally not directly unit-tested in isolation; they are comprehensively verified through
// the Phase 2 Redis CRUD/lifecycle, Lua script transitions, and multi-client concurrency tests.
pub fn lock_hash_key(id: i64) -> String {
    format!("force_join:lock:{id}")
}
pub fn joined_key(lock_id: i64, user_id: i64) -> String {
    format!("force_join:joined:{lock_id}:{user_id}")
}
pub fn counted_key(lock_id: i64, user_id: i64) -> String {
    format!("force_join:counted:{lock_id}:{user_id}")
}
pub fn already_count_key(lock_id: i64) -> String {
    format!("force_join:lock:{lock_id}:already_count")
}
pub fn linked_count_key(lock_id: i64) -> String {
    format!("force_join:lock:{lock_id}:linked_count")
}
