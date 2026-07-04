use tokio_postgres::Client;

use crate::rank::types::Rank;

/// عمر کشویی کد: ۷ روز از آخرین فعالیت (ساخت یا مصرف)
pub const CODE_TTL_SECS: i64 = 7 * 86_400;

/// یک ردیف کد هدیه
pub struct RedeemCodeRow {
    pub rank: Rank,
    pub duration_days: i32,
    #[allow(dead_code)]
    pub max_uses: i32,
    #[allow(dead_code)]
    pub used_count: i32,
    /// زمان انقضای رکورد کد (epoch). None یعنی بدون انقضا (کدهای قدیمی).
    pub expires_at: Option<i64>,
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// ساخت کد جدید در DB (با عمر ۷ روزه)
pub async fn create_code(
    client: &Client,
    code: &str,
    rank: Rank,
    duration_days: i32,
    max_uses: i32,
    created_by: i64,
) -> Result<(), tokio_postgres::Error> {
    let expires_at = now_epoch() + CODE_TTL_SECS;
    client
        .execute(
            "INSERT INTO redeem_codes (code, rank, duration_days, max_uses, created_by, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
            &[&code, &rank.as_str(), &duration_days, &max_uses, &created_by, &expires_at],
        )
        .await?;
    Ok(())
}

/// خواندن کد (None اگر وجود نداشت یا rank نامعتبر بود)
pub async fn get_code(
    client: &Client,
    code: &str,
) -> Result<Option<RedeemCodeRow>, tokio_postgres::Error> {
    let row = client
        .query_opt(
            "SELECT rank, duration_days, max_uses, used_count, expires_at
             FROM redeem_codes WHERE code = $1",
            &[&code],
        )
        .await?;

    let Some(row) = row else { return Ok(None) };

    let rank_str: String = row.get(0);
    let Some(rank) = Rank::from_str(&rank_str) else {
        return Ok(None);
    };

    Ok(Some(RedeemCodeRow {
        rank,
        duration_days: row.get(1),
        max_uses: row.get(2),
        used_count: row.get(3),
        expires_at: row.get(4),
    }))
}

/// زمان مصرف کد توسط این کاربر (epoch)، اگر قبلاً مصرف کرده باشد
pub async fn get_user_redemption(
    client: &Client,
    code: &str,
    user_id: i64,
) -> Result<Option<i64>, tokio_postgres::Error> {
    let row = client
        .query_opt(
            "SELECT redeemed_at FROM redeem_redemptions WHERE code = $1 AND user_id = $2",
            &[&code, &user_id],
        )
        .await?;
    Ok(row.map(|r| r.get::<_, i64>(0)))
}

/// آخرین زمان مصرف کد (برای پیام «ظرفیت پر شد در تاریخ ...»)
pub async fn get_last_redemption(
    client: &Client,
    code: &str,
) -> Result<Option<i64>, tokio_postgres::Error> {
    let row = client
        .query_opt(
            "SELECT MAX(redeemed_at) FROM redeem_redemptions WHERE code = $1",
            &[&code],
        )
        .await?;
    Ok(row.and_then(|r| r.get::<_, Option<i64>>(0)))
}

/// مصرف اتمیک کد: شمارنده را افزایش می‌دهد فقط اگر ظرفیت باقی باشد، مصرف کاربر را ثبت
/// می‌کند و عمر کد را ۷ روز ریست می‌کند. true یعنی موفق، false یعنی ظرفیت تمام.
/// فرض: قبلاً با get_user_redemption بررسی شده که این کاربر تکراری نیست.
pub async fn mark_redeemed(
    client: &Client,
    code: &str,
    user_id: i64,
) -> Result<bool, tokio_postgres::Error> {
    let now = now_epoch();
    let new_expiry = now + CODE_TTL_SECS;
    let updated = client
        .execute(
            "UPDATE redeem_codes SET used_count = used_count + 1, expires_at = $2
             WHERE code = $1 AND used_count < max_uses",
            &[&code, &new_expiry],
        )
        .await?;

    if updated == 0 {
        return Ok(false);
    }

    client
        .execute(
            "INSERT INTO redeem_redemptions (code, user_id, redeemed_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (code, user_id) DO NOTHING",
            &[&code, &user_id, &now],
        )
        .await?;

    Ok(true)
}

/// حذف یک کد و مصرف‌هایش (هنگام انقضای lazy)
pub async fn delete_code(client: &Client, code: &str) -> Result<(), tokio_postgres::Error> {
    client.execute("DELETE FROM redeem_redemptions WHERE code = $1", &[&code]).await?;
    client.execute("DELETE FROM redeem_codes WHERE code = $1", &[&code]).await?;
    Ok(())
}

/// پاک‌سازی دوره‌ای کدهای منقضی‌شده. تعداد کدهای حذف‌شده را برمی‌گرداند.
pub async fn sweep_expired(client: &Client) -> Result<u64, tokio_postgres::Error> {
    let now = now_epoch();
    // اول مصرف‌های یتیم کدهای منقضی، بعد خود کدها
    client
        .execute(
            "DELETE FROM redeem_redemptions WHERE code IN
                (SELECT code FROM redeem_codes WHERE expires_at IS NOT NULL AND expires_at < $1)",
            &[&now],
        )
        .await?;
    let n = client
        .execute(
            "DELETE FROM redeem_codes WHERE expires_at IS NOT NULL AND expires_at < $1",
            &[&now],
        )
        .await?;
    Ok(n)
}
