use redis::aio::MultiplexedConnection;
use serde::{Deserialize, Serialize};

pub const SESSION_TTL_SECS: u64 = 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressSession {
    pub file_id: String,
    pub filename: String,
    pub orig_w: u32,
    pub orig_h: u32,
    pub orig_fps: u32,
    pub orig_bitrate: u64, // in bps from ffprobe
    pub orig_codec: String,
    pub orig_size_bytes: u64,
    pub duration_secs: u64,

    // Current user selections
    pub codec: String, // "h264", "h265", "vp9", "av1"
    pub res_h: u32,    // 2160, 1440, 1080, 720, 480, 360, 240, 144
    pub fps: u32,      // 60, 45, 30, 24, 20, 15, 13
    pub br_ratio: u32, // 100, 75, 50, 25, 16, 12
}

pub fn redis_key(user_id: i64) -> String {
    format!("studio_comp_session:{user_id}")
}

pub async fn redis_conn() -> redis::RedisResult<MultiplexedConnection> {
    let client = redis::Client::open(crate::config::redis_url())?;
    client.get_multiplexed_async_connection().await
}

pub async fn load_session(user_id: i64) -> Option<CompressSession> {
    let Ok(mut c) = redis_conn().await else {
        return None;
    };
    let val: Option<String> = redis::cmd("GET")
        .arg(redis_key(user_id))
        .query_async(&mut c)
        .await
        .ok()
        .flatten();
    val.as_deref().and_then(|s| serde_json::from_str(s).ok())
}

pub async fn save_session(user_id: i64, session: &CompressSession) {
    let Ok(mut c) = redis_conn().await else {
        return;
    };
    if let Ok(json) = serde_json::to_string(session) {
        let _: Result<(), _> = redis::cmd("SET")
            .arg(redis_key(user_id))
            .arg(json)
            .arg("EX")
            .arg(SESSION_TTL_SECS)
            .query_async::<()>(&mut c)
            .await;
    }
}

pub async fn clear_session(user_id: i64) {
    let Ok(mut c) = redis_conn().await else {
        return;
    };
    let _: Result<i64, _> = redis::cmd("DEL")
        .arg(redis_key(user_id))
        .query_async(&mut c)
        .await;
}
