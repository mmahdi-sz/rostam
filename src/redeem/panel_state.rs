//! وضعیت پنل گرافیکی ساخت کد، ذخیره‌شده در Redis (کلید per-admin).
//!
//! کلید: `gencode:state:{admin_id}` → مقدار `"{rank}|{days}|{uses}"`، TTL یک ساعت.
//! انتخاب‌ها بین کلیک‌ها باقی می‌مانند بدون نگه‌داری state در حافظه‌ی پروسه.

use redis::aio::MultiplexedConnection;

use crate::config;
use crate::rank::types::Rank;

const STATE_TTL_SECS: u64 = 3600;

/// انتخاب فعلی ادمین در پنل ساخت کد
#[derive(Debug, Clone, Copy)]
pub struct GenSelection {
    pub rank: Rank,
    pub days: i32,
    pub uses: i32,
}

impl Default for GenSelection {
    /// پیش‌فرض: اسفندیار، ۳۱ روز، ۱ عدد
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

/// اتصال Redis تازه (per-call؛ پنل کم‌تکرار و فقط ادمین است)
async fn conn() -> redis::RedisResult<MultiplexedConnection> {
    let client = redis::Client::open(config::redis_url())?;
    client.get_multiplexed_async_connection().await
}

/// خواندن انتخاب فعلی؛ اگر نبود/خراب بود → پیش‌فرض
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

/// نوشتن انتخاب با TTL یک‌ساعته
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

/// پاک‌کردن state (بعد از ساخت موفق کد)
pub async fn clear(admin_id: i64) {
    let Ok(mut c) = conn().await else { return };
    let _: Result<i64, _> = redis::cmd("DEL")
        .arg(key(admin_id))
        .query_async(&mut c)
        .await;
}
