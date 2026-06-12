pub mod menu;
pub mod paywall;
pub mod quota;
pub mod store;
pub mod types;

use tokio_postgres::Client;

use self::store::get_user_rank;
use self::types::Rank;

/// مقام فعلی کاربر — اگر منقضی شده یا نبود، دلاور برمی‌گرده
pub async fn effective_rank(client: &Client, user_id: i64) -> Rank {
    let Ok(Some(row)) = get_user_rank(client, user_id).await else {
        return Rank::Dalavar;
    };

    if let Some(expires_at) = row.expires_at {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if now > expires_at {
            return Rank::Dalavar;
        }
    }

    row.rank
}
