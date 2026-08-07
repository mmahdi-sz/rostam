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
            &[
                &code,
                &rank.as_str(),
                &duration_days,
                &max_uses,
                &created_by,
                &expires_at,
            ],
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

/// نتیجه‌ی مصرف کد. سه حالت متمایز که پیام کاربر برای هرکدام فرق دارد.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedeemOutcome {
    /// ثبت شد و ظرفیت مصرف شد.
    Consumed,
    /// این کاربر قبلاً همین کد را مصرف کرده بود.
    AlreadyRedeemed,
    /// ظرفیت کد پر بود؛ چیزی مصرف نشد.
    Exhausted,
}

/// نگاشت شمارنده‌های خروجی CTE به نتیجه. جدا شده تا بدون دیتابیس تست‌پذیر باشد.
///
/// `consumed > 0` با این CTE همیشه `inserted > 0` را نتیجه می‌دهد (شرط
/// `EXISTS (SELECT 1 FROM ins)` روی UPDATE)، پس حالت (0, 1) غیرممکن است.
fn classify(inserted: i64, consumed: i64) -> RedeemOutcome {
    match (inserted, consumed) {
        // ردیف مصرف از قبل بود ⇒ این کاربر تکراری است.
        (0, _) => RedeemOutcome::AlreadyRedeemed,
        // ثبت شد ولی شمارنده بالا نرفت ⇒ ظرفیت پر بود.
        (_, 0) => RedeemOutcome::Exhausted,
        _ => RedeemOutcome::Consumed,
    }
}

/// مصرف اتمیک کد: در **یک** statement مصرف کاربر را ثبت می‌کند، شمارنده را فقط
/// اگر آن ثبت واقعاً انجام شده و ظرفیت باقی باشد بالا می‌برد، و عمر کد را ۷ روز
/// ریست می‌کند.
pub async fn mark_redeemed(
    client: &Client,
    code: &str,
    user_id: i64,
) -> Result<RedeemOutcome, tokio_postgres::Error> {
    let now = now_epoch();
    let new_expiry = now + CODE_TTL_SECS;
    // ترتیب مهم است: درج اول می‌آید چون PRIMARY KEY(code, user_id) تنها نقطه‌ی
    // سریالایز شدن دو درخواست همزمان است؛ افزایش شمارنده به همان درج گره خورده.
    // برعکس کردن این ترتیب، باگ «یک کاربر چند ظرفیت را می‌سوزاند» را برمی‌گرداند.
    let row = client
        .query_one(
            "WITH ins AS (
                 INSERT INTO redeem_redemptions (code, user_id, redeemed_at)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (code, user_id) DO NOTHING
                 RETURNING 1
             ), upd AS (
                 UPDATE redeem_codes
                    SET used_count = used_count + 1,
                        expires_at = $4
                  WHERE code = $1
                    AND used_count < max_uses
                    AND EXISTS (SELECT 1 FROM ins)
                 RETURNING 1
             )
             SELECT (SELECT count(*) FROM ins)::bigint AS inserted,
                    (SELECT count(*) FROM upd)::bigint AS consumed",
            &[&code, &user_id, &now, &new_expiry],
        )
        .await?;
    let inserted: i64 = row.get(0);
    let consumed: i64 = row.get(1);

    if inserted == 0 && consumed > 0 {
        // با CTE بالا غیرممکن است؛ اگر دیده شد یعنی SQL دست خورده و مسیر پول
        // دیگر اتمیک نیست. سکوت نمی‌کنیم.
        crate::stats::record_error_global(
            "redeem",
            format!("impossible redeem counts: inserted={inserted} consumed={consumed}"),
        )
        .await;
    }

    let outcome = classify(inserted, consumed);

    if outcome == RedeemOutcome::Exhausted {
        // درج انجام شد ولی ظرفیت نبود. ردیفی که همین درخواست ساخت باید برود،
        // وگرنه کاربر برای همیشه از کدی که هرگز نگرفته محروم می‌ماند (اگر ادمین
        // max_uses را بالا ببرد، «قبلاً مصرف کرده‌ای» می‌گیرد). این DELETE امن
        // است چون ردیف (code, user_id) منحصر به همین درخواست است.
        if let Err(e) = client
            .execute(
                "DELETE FROM redeem_redemptions WHERE code = $1 AND user_id = $2",
                &[&code, &user_id],
            )
            .await
        {
            eprintln!("[redeem event=exhausted_rollback_failed] user_id={user_id} err={e}");
            crate::stats::record_error_global("redeem", format!("exhausted rollback failed: {e}"))
                .await;
        }
    }

    Ok(outcome)
}

/// حذف یک کد و مصرف‌هایش (هنگام انقضای lazy)
pub async fn delete_code(client: &Client, code: &str) -> Result<(), tokio_postgres::Error> {
    client
        .execute("DELETE FROM redeem_redemptions WHERE code = $1", &[&code])
        .await?;
    client
        .execute("DELETE FROM redeem_codes WHERE code = $1", &[&code])
        .await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// سه حالت واقعی که CTE می‌تواند تولید کند. اگر روزی کسی ترتیب `ins`/`upd`
    /// را عوض کرد یا شرط `EXISTS` را برداشت، این تست‌ها همان جایی‌اند که معنی
    /// شمارنده‌ها را ثبت کرده‌اند.
    #[test]
    fn classify_maps_the_three_cte_outcomes() {
        // درج شد + شمارنده بالا رفت ⇒ کد واقعاً به این کاربر داده شد.
        assert_eq!(classify(1, 1), RedeemOutcome::Consumed);
        // درج نشد (PK خورد) ⇒ همین کاربر قبلاً گرفته بود؛ ظرفیت دست نخورده.
        assert_eq!(classify(0, 0), RedeemOutcome::AlreadyRedeemed);
        // درج شد ولی `used_count < max_uses` رد شد ⇒ ظرفیت پر بود.
        assert_eq!(classify(1, 0), RedeemOutcome::Exhausted);
    }

    /// حالت (0, 1) با این SQL غیرممکن است (UPDATE به `EXISTS (ins)` گره خورده).
    /// اگر روزی رخ داد یعنی SQL دست خورده؛ ایمن‌ترین تفسیر «به این کاربر ندادیم»
    /// است، نه «دادیم» — و `mark_redeemed` هم آن را به `record_error_global`
    /// می‌فرستد.
    #[test]
    fn classify_impossible_pair_never_grants() {
        assert_ne!(classify(0, 1), RedeemOutcome::Consumed);
    }

    /// e2e روی دیتابیس dev و از مسیر واقعیِ production: یک `Arc<Client>` مشترک
    /// (همان چیزی که `PostgresDatabase` نگه می‌دارد) و چند تسک همزمان.
    /// چیزی که این تست می‌سنجد و تست واحد نمی‌تواند: ظرفیت ۳تایی زیر ۱۲ کلیک
    /// همزمان دقیقاً ۳ بار داده می‌شود، و یک کاربر با دو کلیک همزمان دو ظرفیت
    /// نمی‌سوزاند.
    ///
    /// `cargo test mark_redeemed_e2e -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn mark_redeemed_e2e_concurrency() {
        use std::sync::Arc;

        const CODE: &str = "TESTE2EATOMIC";
        const BASE_UID: i64 = -999_100;

        let Some(db_url) = crate::config::database_url() else {
            panic!("DATABASE_URL not resolvable from .env — run from the crate root");
        };
        let (client, conn) = tokio_postgres::connect(&db_url, tokio_postgres::NoTls)
            .await
            .expect("dev DB must be reachable");
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let client = Arc::new(client);

        // ساخت کد آزمایشی با ظرفیت ۳ (اجرای مکرر تست باید تمیز شروع شود)
        let reset = |max_uses: i32| {
            let client = client.clone();
            async move {
                client
                    .execute("DELETE FROM redeem_redemptions WHERE code = $1", &[&CODE])
                    .await
                    .expect("cleanup redemptions");
                client
                    .execute("DELETE FROM redeem_codes WHERE code = $1", &[&CODE])
                    .await
                    .expect("cleanup code");
                client
                    .execute(
                        "INSERT INTO redeem_codes
                           (code, rank, duration_days, max_uses, used_count, created_by, expires_at)
                         VALUES ($1, 'Sepahbod', 7, $2, 0, -999000, $3)",
                        &[&CODE, &max_uses, &(now_epoch() + CODE_TTL_SECS)],
                    )
                    .await
                    .expect("insert test code");
            }
        };

        // ۱) ۱۲ کاربر متفاوت، همزمان، روی ظرفیت ۳
        reset(3).await;
        let mut set = tokio::task::JoinSet::new();
        for i in 0..12i64 {
            let client = client.clone();
            set.spawn(async move { mark_redeemed(&client, CODE, BASE_UID - i).await });
        }
        let mut consumed = 0;
        let mut already = 0;
        let mut exhausted = 0;
        while let Some(res) = set.join_next().await {
            match res.expect("task panicked").expect("query failed") {
                RedeemOutcome::Consumed => consumed += 1,
                RedeemOutcome::AlreadyRedeemed => already += 1,
                RedeemOutcome::Exhausted => exhausted += 1,
            }
        }
        assert_eq!(
            (consumed, already, exhausted),
            (3, 0, 9),
            "capacity 3 under 12 concurrent clicks"
        );
        let used: i32 = client
            .query_one(
                "SELECT used_count FROM redeem_codes WHERE code = $1",
                &[&CODE],
            )
            .await
            .expect("read used_count")
            .get(0);
        assert_eq!(used, 3, "used_count drifted from the number of grants");
        let rows: i64 = client
            .query_one(
                "SELECT count(*) FROM redeem_redemptions WHERE code = $1",
                &[&CODE],
            )
            .await
            .expect("count redemptions")
            .get(0);
        assert_eq!(
            rows, 3,
            "exhausted requests left rows behind; rollback did not run"
        );

        // ۲) یک کاربر، دو کلیک همزمان، ظرفیت ۲: فقط یکی باید مصرف شود
        reset(2).await;
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let client = client.clone();
            set.spawn(async move { mark_redeemed(&client, CODE, BASE_UID).await });
        }
        let mut consumed = 0;
        let mut already = 0;
        while let Some(res) = set.join_next().await {
            match res.expect("task panicked").expect("query failed") {
                RedeemOutcome::Consumed => consumed += 1,
                RedeemOutcome::AlreadyRedeemed => already += 1,
                RedeemOutcome::Exhausted => panic!("capacity 2 must not report exhausted"),
            }
        }
        assert_eq!(
            (consumed, already),
            (1, 1),
            "one user's double-tap burned two uses"
        );
        let used: i32 = client
            .query_one(
                "SELECT used_count FROM redeem_codes WHERE code = $1",
                &[&CODE],
            )
            .await
            .expect("read used_count")
            .get(0);
        assert_eq!(used, 1, "double-tap incremented used_count twice");

        // تمیزکاری
        client
            .execute("DELETE FROM redeem_redemptions WHERE code = $1", &[&CODE])
            .await
            .expect("final cleanup redemptions");
        client
            .execute("DELETE FROM redeem_codes WHERE code = $1", &[&CODE])
            .await
            .expect("final cleanup code");
    }
}
