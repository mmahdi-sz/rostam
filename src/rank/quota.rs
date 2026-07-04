use tokio_postgres::Client;

/// نوع quota
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum QuotaKind {
    TrafficDaily,
    TrafficMonthly,
    SttWeekly,
    Upscale2xWeekly,
    Upscale3xWeekly,
    Upscale4xWeekly,
    AiChatMonthly,
    DenoiseDaily,
    DenoiseWeekly,
    SttFastDaily,
    SttFastWeekly,
    SttAccurateDaily,
    SttAccurateWeekly,
    SeparationDaily,
    SeparationWeekly,
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
            Self::AiChatMonthly => "ai_chat_monthly",
            Self::DenoiseDaily => "denoise_daily",
            Self::DenoiseWeekly => "denoise_weekly",
            Self::SttFastDaily => "stt_fast_daily",
            Self::SttFastWeekly => "stt_fast_weekly",
            Self::SttAccurateDaily => "stt_accurate_daily",
            Self::SttAccurateWeekly => "stt_accurate_weekly",
            Self::SeparationDaily => "separation_daily",
            Self::SeparationWeekly => "separation_weekly",
        }
    }
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// شروع روز جاری ساعت ۰۰:۰۰ به وقت تهران (UTC+3:30 = 12600 ثانیه)
fn today_start_tehran() -> i64 {
    const TEHRAN_OFFSET: i64 = 3 * 3600 + 30 * 60;
    let now = now_epoch();
    let local = now + TEHRAN_OFFSET;
    let day_start_local = local - (local % 86_400);
    day_start_local - TEHRAN_OFFSET
}

/// شروع دوره ماهانه جاری بر اساس first_upload_at
/// هر ۳۰ روز یک دوره — آخرین مضرب ۳۰ روز از first_upload_at که قبل از now باشه
fn monthly_window_start(first_upload_at: i64) -> i64 {
    let now = now_epoch();
    let elapsed = now - first_upload_at;
    let cycles = elapsed / (30 * 86_400);
    first_upload_at + cycles * 30 * 86_400
}

/// مصرف ترافیک روزانه (bytes)
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

/// مصرف ترافیک ماهانه (bytes) — نیاز به first_upload_at داره
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

/// خواندن first_upload_at کاربر از stats_users (None اگه هنوز آپلودی نکرده)
pub async fn get_first_upload_at(
    client: &Client,
    user_id: i64,
) -> Option<i64> {
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

/// ثبت ترافیک آپلود موفق — daily + monthly با هم
/// first_upload_at: اولین آپلود موفق کاربر (همین آپلود اگه اولیه)
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

/// مصرف فعلی برای quota‌های غیرترافیکی (weekly/monthly با window ثابت)
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

/// افزایش مصرف برای quota‌های غیرترافیکی
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
