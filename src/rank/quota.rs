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
}
