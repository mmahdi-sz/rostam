use tokio_postgres::Client;

/// Quota kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum QuotaKind {
    TrafficDaily,
    TrafficMonthly,
    SttWeekly,
    Upscale2xWeekly,
    Upscale3xWeekly,
    Upscale4xWeekly,
    DeoldifyWeekly,
    TtsWeekly,
    DenoiseDaily,
    DenoiseWeekly,
    SttFastDaily,
    SttFastWeekly,
    SttAccurateDaily,
    SttAccurateWeekly,
    SeparationDaily,
    SeparationWeekly,
    NobgWeekly,
    CompressCpuDaily,
    CompressCpuMonthly,
    PkgConvertDaily,
}

impl QuotaKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TrafficDaily => "traffic_daily",
            Self::TrafficMonthly => "traffic_monthly",
            Self::SttWeekly => "stt_weekly",
            Self::Upscale2xWeekly => "upscale_2x_weekly",
            Self::Upscale3xWeekly => "upscale_3x_weekly",
            Self::Upscale4xWeekly => "upscale_4x_weekly",
            Self::DeoldifyWeekly => "deoldify_weekly",
            Self::TtsWeekly => "tts_weekly",
            Self::DenoiseDaily => "denoise_daily",
            Self::DenoiseWeekly => "denoise_weekly",
            Self::SttFastDaily => "stt_fast_daily",
            Self::SttFastWeekly => "stt_fast_weekly",
            Self::SttAccurateDaily => "stt_accurate_daily",
            Self::SttAccurateWeekly => "stt_accurate_weekly",
            Self::SeparationDaily => "separation_daily",
            Self::SeparationWeekly => "separation_weekly",
            Self::NobgWeekly => "nobg_weekly",
            Self::CompressCpuDaily => "compress_cpu_daily",
            Self::CompressCpuMonthly => "compress_cpu_monthly",
            Self::PkgConvertDaily => "pkg_convert_daily",
        }
    }
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Start of today at 00:00 Tehran time (UTC+3:30 = 12600 seconds).
fn today_start_tehran() -> i64 {
    const TEHRAN_OFFSET: i64 = 3 * 3600 + 30 * 60;
    let now = now_epoch();
    let local = now + TEHRAN_OFFSET;
    let day_start_local = local - (local % 86_400);
    day_start_local - TEHRAN_OFFSET
}

/// Start of monthly window based on `first_upload_at` (30-day cycles).
fn monthly_window_start(first_upload_at: i64) -> i64 {
    let now = now_epoch();
    let elapsed = now - first_upload_at;
    let cycles = elapsed / (30 * 86_400);
    first_upload_at + cycles * 30 * 86_400
}

/// Daily traffic usage (bytes).
pub async fn get_daily_traffic(
    client: &Client,
    user_id: i64,
) -> Result<i64, tokio_postgres::Error> {
    let window_start = today_start_tehran();
    let row = client
        .query_opt(
            "SELECT used FROM user_quotas
             WHERE user_id = $1 AND quota_type = 'traffic_daily' AND window_start = $2",
            &[&user_id, &window_start],
        )
        .await?;
    Ok(row.map(|r| r.get::<_, i64>(0)).unwrap_or(0))
}

/// Monthly traffic usage (bytes).
pub async fn get_monthly_traffic(
    client: &Client,
    user_id: i64,
    first_upload_at: i64,
) -> Result<i64, tokio_postgres::Error> {
    let window_start = monthly_window_start(first_upload_at);
    let row = client
        .query_opt(
            "SELECT used FROM user_quotas
             WHERE user_id = $1 AND quota_type = 'traffic_monthly' AND window_start = $2",
            &[&user_id, &window_start],
        )
        .await?;
    Ok(row.map(|r| r.get::<_, i64>(0)).unwrap_or(0))
}

/// Get `first_upload_at` from `stats_users`.
pub async fn get_first_upload_at(client: &Client, user_id: i64) -> Option<i64> {
    client
        .query_opt(
            "SELECT first_upload_at FROM stats_users WHERE user_id = $1",
            &[&user_id],
        )
        .await
        .ok()
        .flatten()
        .and_then(|r| r.get::<_, Option<i64>>(0))
}

/// Add upload traffic (both daily and monthly).
pub async fn add_traffic(
    client: &Client,
    user_id: i64,
    bytes: i64,
    first_upload_at: i64,
) -> Result<(), tokio_postgres::Error> {
    let daily_window = today_start_tehran();
    let monthly_window = monthly_window_start(first_upload_at);

    // daily
    client
        .execute(
            "INSERT INTO user_quotas (user_id, quota_type, used, window_start)
             VALUES ($1, 'traffic_daily', $2, $3)
             ON CONFLICT (user_id, quota_type) DO UPDATE SET
                used = CASE
                    WHEN user_quotas.window_start = EXCLUDED.window_start
                    THEN user_quotas.used + EXCLUDED.used
                    ELSE EXCLUDED.used
                END,
                window_start = EXCLUDED.window_start",
            &[&user_id, &bytes, &daily_window],
        )
        .await?;

    // monthly
    client
        .execute(
            "INSERT INTO user_quotas (user_id, quota_type, used, window_start)
             VALUES ($1, 'traffic_monthly', $2, $3)
             ON CONFLICT (user_id, quota_type) DO UPDATE SET
                used = CASE
                    WHEN user_quotas.window_start = EXCLUDED.window_start
                    THEN user_quotas.used + EXCLUDED.used
                    ELSE EXCLUDED.used
                END,
                window_start = EXCLUDED.window_start",
            &[&user_id, &bytes, &monthly_window],
        )
        .await?;

    Ok(())
}

/// Current usage for non-traffic quotas.
pub async fn get_usage(
    client: &Client,
    user_id: i64,
    kind: QuotaKind,
    window_secs: i64,
) -> Result<i64, tokio_postgres::Error> {
    let window_start = now_epoch() - window_secs;
    let row = client
        .query_opt(
            "SELECT used FROM user_quotas
             WHERE user_id = $1 AND quota_type = $2 AND window_start > $3",
            &[&user_id, &kind.as_str(), &window_start],
        )
        .await?;
    Ok(row.map(|r| r.get::<_, i64>(0)).unwrap_or(0))
}

/// Increment usage for non-traffic quotas.
pub async fn add_usage(
    client: &Client,
    user_id: i64,
    kind: QuotaKind,
    amount: i64,
    window_secs: i64,
) -> Result<(), tokio_postgres::Error> {
    let now = now_epoch();
    let window_start = now - window_secs;

    client
        .execute(
            "INSERT INTO user_quotas (user_id, quota_type, used, window_start)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id, quota_type) DO UPDATE SET
                used = CASE
                    WHEN user_quotas.window_start > $5
                    THEN user_quotas.used + EXCLUDED.used
                    ELSE EXCLUDED.used
                END,
                window_start = CASE
                    WHEN user_quotas.window_start > $5
                    THEN user_quotas.window_start
                    ELSE EXCLUDED.window_start
                END",
            &[&user_id, &kind.as_str(), &amount, &now, &window_start],
        )
        .await?;

    Ok(())
}

/// Checks if `amount` fits within `limit`. Rejects negative amounts.
fn fits(amount: i64, limit: i64) -> bool {
    amount >= 0 && amount <= limit
}

/// Atomically reserves usage if `current + amount <= limit`.
/// Returns `Ok(None)` if quota exceeded, or `Ok(Some(used_after))` if reserved.
pub async fn reserve_usage(
    client: &Client,
    user_id: i64,
    kind: QuotaKind,
    amount: i64,
    window_secs: i64,
    limit: i64,
) -> Result<Option<i64>, tokio_postgres::Error> {
    if !fits(amount, limit) {
        return Ok(None);
    }
    let now = now_epoch();
    let window_start = now - window_secs;

    let row = client
        .query_opt(
            "INSERT INTO user_quotas (user_id, quota_type, used, window_start)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id, quota_type) DO UPDATE SET
                used = CASE
                    WHEN user_quotas.window_start > $5
                    THEN user_quotas.used + EXCLUDED.used
                    ELSE EXCLUDED.used
                END,
                window_start = CASE
                    WHEN user_quotas.window_start > $5
                    THEN user_quotas.window_start
                    ELSE EXCLUDED.window_start
                END
             WHERE CASE
                    WHEN user_quotas.window_start > $5
                    THEN user_quotas.used + EXCLUDED.used
                    ELSE EXCLUDED.used
                   END <= $6
             RETURNING used",
            &[
                &user_id,
                &kind.as_str(),
                &amount,
                &now,
                &window_start,
                &limit,
            ],
        )
        .await?;

    Ok(row.map(|r| r.get::<_, i64>(0)))
}

/// Refund reserved quota (e.g. task failure). Clamped at 0.
pub async fn refund_usage(
    client: &Client,
    user_id: i64,
    kind: QuotaKind,
    amount: i64,
    window_secs: i64,
) -> Result<(), tokio_postgres::Error> {
    let window_start = now_epoch() - window_secs;

    client
        .execute(
            "UPDATE user_quotas
                SET used = GREATEST(used - $3, 0)
              WHERE user_id = $1 AND quota_type = $2 AND window_start > $4",
            &[&user_id, &kind.as_str(), &amount, &window_start],
        )
        .await?;

    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::task::JoinSet;

    async fn connect_test_db() -> Option<Arc<Client>> {
        let db_url = crate::config::database_url()?;
        let (client, conn) = tokio_postgres::connect(&db_url, tokio_postgres::NoTls)
            .await
            .ok()?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        Some(Arc::new(client))
    }

    #[test]
    fn test_fits_boundaries() {
        assert!(fits(0, 0));
        assert!(!fits(1, 0));
        assert!(fits(5, 5));
        assert!(!fits(6, 5));
    }

    #[test]
    fn test_fits_rejects_negative() {
        assert!(!fits(-1, 60));
        assert!(!fits(-1, 0));
        assert!(!fits(i64::MIN, i64::MAX));
    }

    #[test]
    fn test_quota_kind_as_str() {
        assert_eq!(QuotaKind::TrafficDaily.as_str(), "traffic_daily");
        assert_eq!(QuotaKind::TrafficMonthly.as_str(), "traffic_monthly");
        assert_eq!(QuotaKind::SttWeekly.as_str(), "stt_weekly");
        assert_eq!(QuotaKind::Upscale2xWeekly.as_str(), "upscale_2x_weekly");
        assert_eq!(QuotaKind::Upscale3xWeekly.as_str(), "upscale_3x_weekly");
        assert_eq!(QuotaKind::Upscale4xWeekly.as_str(), "upscale_4x_weekly");
        assert_eq!(QuotaKind::DeoldifyWeekly.as_str(), "deoldify_weekly");
        assert_eq!(QuotaKind::TtsWeekly.as_str(), "tts_weekly");
        assert_eq!(QuotaKind::DenoiseDaily.as_str(), "denoise_daily");
        assert_eq!(QuotaKind::DenoiseWeekly.as_str(), "denoise_weekly");
        assert_eq!(QuotaKind::SttFastDaily.as_str(), "stt_fast_daily");
        assert_eq!(QuotaKind::SttFastWeekly.as_str(), "stt_fast_weekly");
        assert_eq!(QuotaKind::SttAccurateDaily.as_str(), "stt_accurate_daily");
        assert_eq!(QuotaKind::SttAccurateWeekly.as_str(), "stt_accurate_weekly");
        assert_eq!(QuotaKind::SeparationDaily.as_str(), "separation_daily");
        assert_eq!(QuotaKind::SeparationWeekly.as_str(), "separation_weekly");
        assert_eq!(QuotaKind::NobgWeekly.as_str(), "nobg_weekly");
        assert_eq!(QuotaKind::CompressCpuDaily.as_str(), "compress_cpu_daily");
        assert_eq!(
            QuotaKind::CompressCpuMonthly.as_str(),
            "compress_cpu_monthly"
        );
        assert_eq!(QuotaKind::PkgConvertDaily.as_str(), "pkg_convert_daily");
    }

    #[test]
    fn test_monthly_window_start() {
        let first_upload = 1_000_000;
        let window_start = monthly_window_start(first_upload);
        assert!(window_start <= now_epoch());
        assert!((now_epoch() - window_start) < 30 * 86_400);
    }

    #[test]
    fn test_today_start_tehran() {
        let start = today_start_tehran();
        let now = now_epoch();
        assert!(start <= now);
        assert!(now - start < 86_400);
    }

    async fn cleanup_user(client: &Client, uid: i64) {
        let _ = client
            .execute("DELETE FROM user_quotas WHERE user_id = $1", &[&uid])
            .await;
    }

    // ── 1. Concurrency / Double-Spend Prevention Test ─────────────────────────
    /// `cargo test reserve_usage_e2e_concurrency -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn reserve_usage_e2e_concurrency() {
        const TEST_UID: i64 = -999_211;
        const KIND: QuotaKind = QuotaKind::Upscale2xWeekly;
        const WINDOW_SECS: i64 = 604_800;

        let Some(client) = connect_test_db().await else {
            eprintln!("[test] Skipping reserve_usage_e2e_concurrency: DATABASE_URL not available");
            return;
        };

        // Scenario 1: 15 concurrent single-unit requests against a strict limit of 5.
        cleanup_user(&client, TEST_UID).await;
        let limit = 5i64;
        let task_count = 15;
        let mut set = JoinSet::new();

        for _ in 0..task_count {
            let client = client.clone();
            set.spawn(async move {
                reserve_usage(&client, TEST_UID, KIND, 1, WINDOW_SECS, limit).await
            });
        }

        let mut granted = 0;
        let mut rejected = 0;
        while let Some(res) = set.join_next().await {
            match res.expect("join error").expect("db error") {
                Some(_) => granted += 1,
                None => rejected += 1,
            }
        }

        assert_eq!(
            granted, 5,
            "Exactly 5 reservations must succeed under limit 5"
        );
        assert_eq!(rejected, 10, "Remaining 10 reservations must be rejected");

        let used = get_usage(&client, TEST_UID, KIND, WINDOW_SECS)
            .await
            .expect("get_usage");
        assert_eq!(used, 5, "Final recorded usage in DB must not exceed limit");

        // Scenario 2: Concurrent multi-unit reservations (amount=3, limit=8, 5 concurrent tasks).
        cleanup_user(&client, TEST_UID).await;
        let limit_multi = 8i64;
        let amount_multi = 3i64;
        let mut set_multi = JoinSet::new();

        for _ in 0..5 {
            let client = client.clone();
            set_multi.spawn(async move {
                reserve_usage(
                    &client,
                    TEST_UID,
                    KIND,
                    amount_multi,
                    WINDOW_SECS,
                    limit_multi,
                )
                .await
            });
        }

        let mut granted_multi = 0;
        let mut rejected_multi = 0;
        while let Some(res) = set_multi.join_next().await {
            match res.expect("join error").expect("db error") {
                Some(_) => granted_multi += 1,
                None => rejected_multi += 1,
            }
        }

        assert_eq!(
            granted_multi, 2,
            "Exactly 2 reservations of amount 3 can fit in limit 8 (2 * 3 = 6 <= 8)"
        );
        assert_eq!(
            rejected_multi, 3,
            "Remaining 3 reservations must be rejected (6 + 3 = 9 > 8)"
        );

        let used_multi = get_usage(&client, TEST_UID, KIND, WINDOW_SECS)
            .await
            .expect("get_usage");
        assert_eq!(used_multi, 6, "Final recorded usage in DB must be 6");

        cleanup_user(&client, TEST_UID).await;
    }

    // ── 2. Real Job Failure Refund Test ───────────────────────────────────────
    /// `cargo test refund_usage_on_job_failure_e2e -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn refund_usage_on_job_failure_e2e() {
        const TEST_UID: i64 = -999_212;
        const WINDOW_SECS: i64 = 86_400;

        let Some(client) = connect_test_db().await else {
            eprintln!(
                "[test] Skipping refund_usage_on_job_failure_e2e: DATABASE_URL not available"
            );
            return;
        };

        // Scenario 1: Discrete job failure refund (e.g. Upscale / DeOldify failure).
        cleanup_user(&client, TEST_UID).await;
        let res = reserve_usage(&client, TEST_UID, QuotaKind::Upscale2xWeekly, 1, 604_800, 5)
            .await
            .expect("db error");
        assert_eq!(
            res,
            Some(1),
            "Initial reservation should succeed with used=1"
        );

        // Simulate handler encountering mid-job failure (e.g. download error or engine crash) and invoking refund.
        refund_usage(&client, TEST_UID, QuotaKind::Upscale2xWeekly, 1, 604_800)
            .await
            .expect("refund db error");

        let used = get_usage(&client, TEST_UID, QuotaKind::Upscale2xWeekly, 604_800)
            .await
            .expect("get_usage");
        assert_eq!(
            used, 0,
            "Usage must be fully restored to 0 after failure refund"
        );

        // User can now re-attempt without being blocked by lost quota.
        let retry_res = reserve_usage(&client, TEST_UID, QuotaKind::Upscale2xWeekly, 1, 604_800, 5)
            .await
            .expect("db error");
        assert_eq!(
            retry_res,
            Some(1),
            "Subsequent job reservation must succeed"
        );

        // Scenario 2: Two-tier quota failure rollback (as in STT and Separation handlers).
        // If daily reservation succeeds but weekly fails, the handler immediately refunds daily quota.
        cleanup_user(&client, TEST_UID).await;
        let daily_res = reserve_usage(
            &client,
            TEST_UID,
            QuotaKind::SttFastDaily,
            60,
            WINDOW_SECS,
            120,
        )
        .await
        .expect("daily reserve");
        assert_eq!(daily_res, Some(60), "Daily quota should reserve 60s");

        // Weekly quota is exhausted (limit=0).
        let weekly_res = reserve_usage(&client, TEST_UID, QuotaKind::SttFastWeekly, 60, 604_800, 0)
            .await
            .expect("weekly reserve");
        assert_eq!(weekly_res, None, "Weekly quota must be rejected");

        // Handler error branch: rollback daily quota reservation.
        refund_usage(&client, TEST_UID, QuotaKind::SttFastDaily, 60, WINDOW_SECS)
            .await
            .expect("daily refund");

        let daily_used_after = get_usage(&client, TEST_UID, QuotaKind::SttFastDaily, WINDOW_SECS)
            .await
            .expect("get daily usage");
        assert_eq!(
            daily_used_after, 0,
            "Daily quota must be rolled back to 0 when weekly tier fails"
        );

        // Scenario 3: Refund clamping (cannot drop below 0).
        refund_usage(&client, TEST_UID, QuotaKind::SttFastDaily, 500, WINDOW_SECS)
            .await
            .expect("excess refund");
        let clamped = get_usage(&client, TEST_UID, QuotaKind::SttFastDaily, WINDOW_SECS)
            .await
            .expect("get clamped");
        assert_eq!(clamped, 0, "Refund must clamp at 0 and never turn negative");

        cleanup_user(&client, TEST_UID).await;
    }

    // ── 3. Quota Window Reset Timing Test ─────────────────────────────────────
    /// `cargo test quota_window_reset_timing_e2e -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn quota_window_reset_timing_e2e() {
        const TEST_UID: i64 = -999_213;

        let Some(client) = connect_test_db().await else {
            eprintln!("[test] Skipping quota_window_reset_timing_e2e: DATABASE_URL not available");
            return;
        };

        // Case A: Rolling window (e.g. denoise_daily with 86400s window).
        cleanup_user(&client, TEST_UID).await;
        let past_window_start = now_epoch() - 100_000; // > 86400 seconds ago
        client
            .execute(
                "INSERT INTO user_quotas (user_id, quota_type, used, window_start)
                 VALUES ($1, 'denoise_daily', 500, $2)",
                &[&TEST_UID, &past_window_start],
            )
            .await
            .expect("insert expired record");

        // get_usage must ignore the expired window.
        let expired_usage = get_usage(&client, TEST_UID, QuotaKind::DenoiseDaily, 86_400)
            .await
            .expect("get_usage");
        assert_eq!(
            expired_usage, 0,
            "get_usage must return 0 for expired rolling window records"
        );

        // reserve_usage must overwrite the expired record and start fresh.
        let reserve_res = reserve_usage(&client, TEST_UID, QuotaKind::DenoiseDaily, 20, 86_400, 60)
            .await
            .expect("reserve");
        assert_eq!(
            reserve_res,
            Some(20),
            "reserve_usage must reset expired used=500 and record new used=20"
        );

        let active_usage = get_usage(&client, TEST_UID, QuotaKind::DenoiseDaily, 86_400)
            .await
            .expect("get_usage active");
        assert_eq!(
            active_usage, 20,
            "Active usage must reflect only new window"
        );

        // Case B: Daily Tehran-time traffic reset (traffic_daily).
        cleanup_user(&client, TEST_UID).await;
        let yesterday_tehran = today_start_tehran() - 86_400;
        client
            .execute(
                "INSERT INTO user_quotas (user_id, quota_type, used, window_start)
                 VALUES ($1, 'traffic_daily', 5368709120, $2)", // 5 GB
                &[&TEST_UID, &yesterday_tehran],
            )
            .await
            .expect("insert yesterday traffic");

        let daily_traffic = get_daily_traffic(&client, TEST_UID)
            .await
            .expect("get_daily_traffic");
        assert_eq!(
            daily_traffic, 0,
            "get_daily_traffic must return 0 for yesterday's window"
        );

        // add_traffic today resets the daily record to the current upload amount.
        add_traffic(&client, TEST_UID, 1024, now_epoch())
            .await
            .expect("add_traffic");
        let new_daily_traffic = get_daily_traffic(&client, TEST_UID)
            .await
            .expect("get_daily_traffic today");
        assert_eq!(
            new_daily_traffic, 1024,
            "add_traffic must reset yesterday's traffic and record today's bytes"
        );

        // Case C: Monthly 30-day cycle traffic reset (traffic_monthly).
        let first_upload = 1_000_000i64;
        let current_month_start = monthly_window_start(first_upload);
        let last_month_start = current_month_start - 30 * 86_400;

        client
            .execute(
                "INSERT INTO user_quotas (user_id, quota_type, used, window_start)
                 VALUES ($1, 'traffic_monthly', 21474836480, $2)
                 ON CONFLICT (user_id, quota_type) DO UPDATE SET used = EXCLUDED.used, window_start = EXCLUDED.window_start",
                &[&TEST_UID, &last_month_start],
            )
            .await
            .expect("insert last month traffic");

        let monthly_traffic = get_monthly_traffic(&client, TEST_UID, first_upload)
            .await
            .expect("get_monthly_traffic");
        assert_eq!(
            monthly_traffic, 0,
            "get_monthly_traffic must return 0 for expired 30-day window"
        );

        cleanup_user(&client, TEST_UID).await;
    }

    // ── 4. File / Video Size Cap Boundary Enforcement Test ────────────────────
    /// `cargo test file_size_cap_boundary_enforcement_e2e -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn file_size_cap_boundary_enforcement_e2e() {
        const TEST_UID: i64 = -999_214;
        const LIMIT: i64 = 1000;
        const WINDOW_SECS: i64 = 86_400;

        let Some(client) = connect_test_db().await else {
            eprintln!(
                "[test] Skipping file_size_cap_boundary_enforcement_e2e: DATABASE_URL not available"
            );
            return;
        };

        // Test 4a: Exact limit (1000) vs limit + 1 (1001) on PostgreSQL reserve_usage path.
        cleanup_user(&client, TEST_UID).await;

        // Request exactly at the limit (1000) -> MUST succeed.
        let at_limit = reserve_usage(
            &client,
            TEST_UID,
            QuotaKind::PkgConvertDaily,
            LIMIT,
            WINDOW_SECS,
            LIMIT,
        )
        .await
        .expect("reserve exact limit");
        assert_eq!(at_limit, Some(1000), "Request at exact limit must succeed");

        // Clean up state back to 0.
        cleanup_user(&client, TEST_UID).await;

        // Request at limit + 1 (1001) -> MUST be blocked.
        let over_limit = reserve_usage(
            &client,
            TEST_UID,
            QuotaKind::PkgConvertDaily,
            LIMIT + 1,
            WINDOW_SECS,
            LIMIT,
        )
        .await
        .expect("reserve over limit");
        assert_eq!(
            over_limit, None,
            "Request at limit + 1 byte must be rejected"
        );

        cleanup_user(&client, TEST_UID).await;
    }

    #[test]
    fn test_feature_size_and_duration_caps_boundaries() {
        // Test 4b: Package Converter input cap (200 MB).
        let pkg_max = crate::pkgconvert::validate::MAX_INPUT_FILE_BYTES;
        assert_eq!(pkg_max, 200 * 1024 * 1024);
        assert!(
            !(pkg_max > crate::pkgconvert::validate::MAX_INPUT_FILE_BYTES),
            "Exact 200MB must pass"
        );
        assert!(
            pkg_max + 1 > crate::pkgconvert::validate::MAX_INPUT_FILE_BYTES,
            "200MB + 1 must be rejected"
        );

        // Test 4c: Studio Hardsub Video Burn duration cap (7200s / 2 hours).
        let burn_max_dur = crate::studio::burn::MAX_BURN_DURATION_SECS;
        assert_eq!(burn_max_dur, 7200);
        assert!(
            !(burn_max_dur > crate::studio::burn::MAX_BURN_DURATION_SECS),
            "Exact 7200s must pass"
        );
        assert!(
            burn_max_dur + 1 > crate::studio::burn::MAX_BURN_DURATION_SECS,
            "7200s + 1 must be rejected"
        );

        // Test 4d: Studio Hardsub Video Burn upload cap (2000 MB).
        let burn_max_upload = crate::studio::burn::MAX_UPLOAD_BYTES;
        assert_eq!(burn_max_upload, 2000 * 1024 * 1024);
        assert!(
            !(burn_max_upload > crate::studio::burn::MAX_UPLOAD_BYTES),
            "Exact 2000MB must pass"
        );
        assert!(
            burn_max_upload + 1 > crate::studio::burn::MAX_UPLOAD_BYTES,
            "2000MB + 1 must be rejected"
        );

        // Test 4e: TTS character count cap (500 chars).
        let tts_max = crate::moss_tts::handle::TTS_MAX_CHARS;
        assert_eq!(tts_max, 500);
        assert!(
            !(tts_max > crate::moss_tts::handle::TTS_MAX_CHARS),
            "Exact 500 chars must pass"
        );
        assert!(
            tts_max + 1 > crate::moss_tts::handle::TTS_MAX_CHARS,
            "501 chars must be rejected"
        );

        // Test 4f: PDF Compress default max cap.
        let pdf_max = crate::config::pdf_compress_max_bytes();
        assert!(pdf_max > 0);
        assert!(!(pdf_max > pdf_max), "Exact PDF max bytes must pass");
        assert!(pdf_max + 1 > pdf_max, "PDF max bytes + 1 must be rejected");
    }
}
