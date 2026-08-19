use tokio_postgres::{Client, GenericClient};

use crate::rank::types::Rank;

/// Code TTL sliding window: 7 days from last activity (creation or redemption).
pub const CODE_TTL_SECS: i64 = 7 * 86_400;

/// Redeem code record.
pub struct RedeemCodeRow {
    pub rank: Rank,
    pub duration_days: i32,
    #[allow(dead_code)]
    pub max_uses: i32,
    #[allow(dead_code)]
    pub used_count: i32,
    /// Expiration timestamp (`None` means permanent / legacy code).
    pub expires_at: Option<i64>,
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Creates new code in DB (with 7-day TTL).
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

/// Get code details (`None` if missing or invalid rank).
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

/// Redemption timestamp for specified user.
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

/// Latest redemption timestamp for code.
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

/// Code redemption outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedeemOutcome {
    /// Redeemed successfully.
    Consumed,
    /// Code already used by this user.
    AlreadyRedeemed,
    /// Code max usage limit reached.
    Exhausted,
}

/// Maps CTE counts to `RedeemOutcome`.
fn classify(inserted: i64, consumed: i64) -> RedeemOutcome {
    match (inserted, consumed) {
        (0, _) => RedeemOutcome::AlreadyRedeemed,
        (_, 0) => RedeemOutcome::Exhausted,
        _ => RedeemOutcome::Consumed,
    }
}

/// Atomically redeems code for user.
pub async fn mark_redeemed(
    client: &(impl GenericClient + ?Sized),
    code: &str,
    user_id: i64,
) -> Result<RedeemOutcome, tokio_postgres::Error> {
    let now = now_epoch();
    let new_expiry = now + CODE_TTL_SECS;
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
        crate::stats::record_error_global(
            "redeem",
            format!("impossible redeem counts: inserted={inserted} consumed={consumed}"),
        )
        .await;
    }

    let outcome = classify(inserted, consumed);

    if outcome == RedeemOutcome::Exhausted {
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

/// Active codes list for admin panel.
#[allow(dead_code)]
pub async fn list_active_codes(
    client: &Client,
) -> Result<Vec<RedeemCodeRow>, tokio_postgres::Error> {
    let rows = client
        .query(
            "SELECT rank, duration_days, max_uses, used_count, expires_at
             FROM redeem_codes
             WHERE expires_at IS NULL OR expires_at > EXTRACT(EPOCH FROM NOW())::BIGINT
             ORDER BY expires_at ASC NULLS LAST",
            &[],
        )
        .await?;

    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let rank_str: String = row.get(0);
            let rank = Rank::from_str(&rank_str)?;
            Some(RedeemCodeRow {
                rank,
                duration_days: row.get(1),
                max_uses: row.get(2),
                used_count: row.get(3),
                expires_at: row.get(4),
            })
        })
        .collect())
}

/// Deletes specific code.
pub async fn delete_code(
    client: &Client,
    code: &str,
) -> Result<(), tokio_postgres::Error> {
    client
        .execute(
            "DELETE FROM redeem_redemptions WHERE code = $1",
            &[&code],
        )
        .await?;
    client
        .execute("DELETE FROM redeem_codes WHERE code = $1", &[&code])
        .await?;
    Ok(())
}

/// Sweeps expired codes. Returns count of deleted codes.
pub async fn sweep_expired(client: &mut Client) -> Result<u64, tokio_postgres::Error> {
    let now = now_epoch();
    let txn = client.transaction().await?;
    txn.execute(
        "DELETE FROM redeem_redemptions WHERE code IN
            (SELECT code FROM redeem_codes WHERE expires_at IS NOT NULL AND expires_at < $1)",
        &[&now],
    )
    .await?;
    let n = txn
        .execute(
            "DELETE FROM redeem_codes WHERE expires_at IS NOT NULL AND expires_at < $1",
            &[&now],
        )
        .await?;
    txn.commit().await?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real CTE outcome scenarios. Preserves counter semantics if CTE logic
    /// or `EXISTS` conditions are modified.
    #[test]
    fn classify_maps_the_three_cte_outcomes() {
        // Inserted + counter incremented => code granted to user.
        assert_eq!(classify(1, 1), RedeemOutcome::Consumed);
        // Not inserted (PK clash) => code already claimed by user; capacity untouched.
        assert_eq!(classify(0, 0), RedeemOutcome::AlreadyRedeemed);
        // Inserted but `used_count < max_uses` check failed => capacity exhausted.
        assert_eq!(classify(1, 0), RedeemOutcome::Exhausted);
    }

    /// The (0, 1) pair is impossible with current SQL (UPDATE is tied to `EXISTS (ins)`).
    /// If it occurs, safest interpretation is "not granted" to prevent over-granting.
    #[test]
    fn classify_impossible_pair_never_grants() {
        assert_ne!(classify(0, 1), RedeemOutcome::Consumed);
    }

    /// E2E test on dev database under production setup: shared `Arc<Client>` and concurrent tasks.
    /// Verifies capacity 3 under 12 concurrent clicks yields exactly 3 grants,
    /// and a single user's double-click burns only 1 use.
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

        // Seed test code with capacity 3 (clean state on repeated test runs).
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

        // 1) 12 distinct users concurrently hitting capacity 3
        reset(3).await;
        let mut set = tokio::task::JoinSet::new();
        for i in 0..12i64 {
            let client = client.clone();
            set.spawn(async move { mark_redeemed(&*client, CODE, BASE_UID - i).await });
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

        // 2) Single user, 2 concurrent clicks, capacity 2: only 1 must be consumed
        reset(2).await;
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let client = client.clone();
            set.spawn(async move { mark_redeemed(&*client, CODE, BASE_UID).await });
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

        // Cleanup
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
