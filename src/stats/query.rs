use tokio_postgres::Client;

pub struct UserStats {
    pub total: i64,
    pub new_1d: i64,
    pub new_3d: i64,
    pub new_7d: i64,
    pub new_30d: i64,
}

pub struct DownloadStats {
    pub requests_1d: i64,
    pub requests_3d: i64,
    pub requests_7d: i64,
    pub requests_30d: i64,

    pub bytes_downloaded_1d: i64,
    pub bytes_downloaded_3d: i64,
    pub bytes_downloaded_7d: i64,
    pub bytes_downloaded_30d: i64,

    pub uploads_ok_1d: i64,
    pub uploads_ok_3d: i64,
    pub uploads_ok_7d: i64,
    pub uploads_ok_30d: i64,

    pub bytes_uploaded_1d: i64,
    pub bytes_uploaded_3d: i64,
    pub bytes_uploaded_7d: i64,
    pub bytes_uploaded_30d: i64,
}

pub async fn get_user_stats(client: &Client) -> Result<UserStats, tokio_postgres::Error> {
    let row = client
        .query_one(
            "SELECT
            COUNT(*)                                                      AS total,
            COUNT(*) FILTER (WHERE first_seen >= NOW() - INTERVAL '1 day')   AS new_1d,
            COUNT(*) FILTER (WHERE first_seen >= NOW() - INTERVAL '3 days')  AS new_3d,
            COUNT(*) FILTER (WHERE first_seen >= NOW() - INTERVAL '7 days')  AS new_7d,
            COUNT(*) FILTER (WHERE first_seen >= NOW() - INTERVAL '30 days') AS new_30d
         FROM stats_users",
            &[],
        )
        .await?;

    Ok(UserStats {
        total: row.get::<_, i64>(0),
        new_1d: row.get::<_, i64>(1),
        new_3d: row.get::<_, i64>(2),
        new_7d: row.get::<_, i64>(3),
        new_30d: row.get::<_, i64>(4),
    })
}

pub async fn get_download_stats(client: &Client) -> Result<DownloadStats, tokio_postgres::Error> {
    let row = client.query_one(
        "SELECT
            COUNT(*) FILTER (WHERE created_at >= NOW() - INTERVAL '1 day')   AS req_1d,
            COUNT(*) FILTER (WHERE created_at >= NOW() - INTERVAL '3 days')  AS req_3d,
            COUNT(*) FILTER (WHERE created_at >= NOW() - INTERVAL '7 days')  AS req_7d,
            COUNT(*) FILTER (WHERE created_at >= NOW() - INTERVAL '30 days') AS req_30d,

            COALESCE(SUM(bytes_downloaded) FILTER (WHERE created_at >= NOW() - INTERVAL '1 day'),   0)::BIGINT AS dl_1d,
            COALESCE(SUM(bytes_downloaded) FILTER (WHERE created_at >= NOW() - INTERVAL '3 days'),  0)::BIGINT AS dl_3d,
            COALESCE(SUM(bytes_downloaded) FILTER (WHERE created_at >= NOW() - INTERVAL '7 days'),  0)::BIGINT AS dl_7d,
            COALESCE(SUM(bytes_downloaded) FILTER (WHERE created_at >= NOW() - INTERVAL '30 days'), 0)::BIGINT AS dl_30d,

            COUNT(*) FILTER (WHERE upload_ok AND created_at >= NOW() - INTERVAL '1 day')   AS up_ok_1d,
            COUNT(*) FILTER (WHERE upload_ok AND created_at >= NOW() - INTERVAL '3 days')  AS up_ok_3d,
            COUNT(*) FILTER (WHERE upload_ok AND created_at >= NOW() - INTERVAL '7 days')  AS up_ok_7d,
            COUNT(*) FILTER (WHERE upload_ok AND created_at >= NOW() - INTERVAL '30 days') AS up_ok_30d,

            COALESCE(SUM(bytes_uploaded) FILTER (WHERE created_at >= NOW() - INTERVAL '1 day'),   0)::BIGINT AS up_1d,
            COALESCE(SUM(bytes_uploaded) FILTER (WHERE created_at >= NOW() - INTERVAL '3 days'),  0)::BIGINT AS up_3d,
            COALESCE(SUM(bytes_uploaded) FILTER (WHERE created_at >= NOW() - INTERVAL '7 days'),  0)::BIGINT AS up_7d,
            COALESCE(SUM(bytes_uploaded) FILTER (WHERE created_at >= NOW() - INTERVAL '30 days'), 0)::BIGINT AS up_30d
         FROM stats_downloads",
        &[],
    ).await?;

    Ok(DownloadStats {
        requests_1d: row.get::<_, i64>(0),
        requests_3d: row.get::<_, i64>(1),
        requests_7d: row.get::<_, i64>(2),
        requests_30d: row.get::<_, i64>(3),

        bytes_downloaded_1d: row.get::<_, i64>(4),
        bytes_downloaded_3d: row.get::<_, i64>(5),
        bytes_downloaded_7d: row.get::<_, i64>(6),
        bytes_downloaded_30d: row.get::<_, i64>(7),

        uploads_ok_1d: row.get::<_, i64>(8),
        uploads_ok_3d: row.get::<_, i64>(9),
        uploads_ok_7d: row.get::<_, i64>(10),
        uploads_ok_30d: row.get::<_, i64>(11),

        bytes_uploaded_1d: row.get::<_, i64>(12),
        bytes_uploaded_3d: row.get::<_, i64>(13),
        bytes_uploaded_7d: row.get::<_, i64>(14),
        bytes_uploaded_30d: row.get::<_, i64>(15),
    })
}

// ── per-feature event stats ─────────────────────────────────────────────────────

// چهار بازه‌ی استاندارد پنل آمار.
pub struct Periods {
    pub d1: i64,
    pub d3: i64,
    pub d7: i64,
    pub d30: i64,
}

pub struct FeatureStats {
    pub ok: Periods,     // تعداد رویداد موفق
    pub fail: Periods,   // تعداد رویداد ناموفق (هر چیزی جز ok: fail/timeout/...)
    pub amount: Periods, // مجموع amount روی رویدادهای موفق (ثانیه یا تعداد، بسته به فیچر)
}

// آمار یک فیچر از stats_events. feature مثل "stt" / "denoise" / "asr" ...
pub async fn get_feature_stats(
    client: &Client,
    feature: &str,
) -> Result<FeatureStats, tokio_postgres::Error> {
    let row = client.query_one(
        "SELECT
            COUNT(*) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '1 day')   AS ok_1d,
            COUNT(*) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '3 days')  AS ok_3d,
            COUNT(*) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '7 days')  AS ok_7d,
            COUNT(*) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '30 days') AS ok_30d,

            COUNT(*) FILTER (WHERE status <> 'ok' AND created_at >= NOW() - INTERVAL '1 day')   AS fail_1d,
            COUNT(*) FILTER (WHERE status <> 'ok' AND created_at >= NOW() - INTERVAL '3 days')  AS fail_3d,
            COUNT(*) FILTER (WHERE status <> 'ok' AND created_at >= NOW() - INTERVAL '7 days')  AS fail_7d,
            COUNT(*) FILTER (WHERE status <> 'ok' AND created_at >= NOW() - INTERVAL '30 days') AS fail_30d,

            COALESCE(SUM(amount) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '1 day'),   0)::BIGINT AS amt_1d,
            COALESCE(SUM(amount) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '3 days'),  0)::BIGINT AS amt_3d,
            COALESCE(SUM(amount) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '7 days'),  0)::BIGINT AS amt_7d,
            COALESCE(SUM(amount) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '30 days'), 0)::BIGINT AS amt_30d
         FROM stats_events WHERE feature = $1",
        &[&feature],
    ).await?;

    Ok(FeatureStats {
        ok: Periods {
            d1: row.get(0),
            d3: row.get(1),
            d7: row.get(2),
            d30: row.get(3),
        },
        fail: Periods {
            d1: row.get(4),
            d3: row.get(5),
            d7: row.get(6),
            d30: row.get(7),
        },
        amount: Periods {
            d1: row.get(8),
            d3: row.get(9),
            d7: row.get(10),
            d30: row.get(11),
        },
    })
}

// ── active users ────────────────────────────────────────────────────────────────

pub struct ActiveUsers {
    pub dau: i64,            // فعال در ۲۴ ساعت گذشته (last_seen)
    pub wau: i64,            // فعال در ۷ روز گذشته
    pub returning_1d: i64,   // کاربران غیرجدیدِ فعال امروز (first_seen > 1 روز پیش)
    pub top_feature: String, // پرمصرف‌ترین فیچر AI در ۷ روز (raw key؛ خالی اگر نبود)
    pub top_feature_count: i64,
}

pub async fn get_active_users(client: &Client) -> Result<ActiveUsers, tokio_postgres::Error> {
    let row = client
        .query_one(
            "SELECT
            COUNT(*) FILTER (WHERE last_seen >= NOW() - INTERVAL '1 day')  AS dau,
            COUNT(*) FILTER (WHERE last_seen >= NOW() - INTERVAL '7 days') AS wau,
            COUNT(*) FILTER (WHERE last_seen >= NOW() - INTERVAL '1 day'
                             AND first_seen < NOW() - INTERVAL '1 day')    AS returning_1d
         FROM stats_users",
            &[],
        )
        .await?;

    let top = client
        .query_opt(
            "SELECT feature, COUNT(*)::BIGINT AS c
         FROM stats_events
         WHERE created_at >= NOW() - INTERVAL '7 days'
         GROUP BY feature
         ORDER BY c DESC
         LIMIT 1",
            &[],
        )
        .await?;

    let (top_feature, top_feature_count) = match top {
        Some(r) => (r.get::<_, String>(0), r.get::<_, i64>(1)),
        None => (String::new(), 0),
    };

    Ok(ActiveUsers {
        dau: row.get(0),
        wau: row.get(1),
        returning_1d: row.get(2),
        top_feature,
        top_feature_count,
    })
}

// ── action/status breakdown (برای «آمار بیشتر») ──────────────────────────────────

pub struct ActionCount {
    pub action: String,
    pub status: String,
    pub d1: i64,
    pub d7: i64,
    pub d30: i64,
}

// تفکیک رویدادهای یک فیچر بر اساس (action, status) در ۳۰ روز اخیر، مرتب نزولی بر حسب ۳۰d.
pub async fn get_action_breakdown(
    client: &Client,
    feature: &str,
) -> Result<Vec<ActionCount>, tokio_postgres::Error> {
    let rows = client
        .query(
            "SELECT action, status,
            COUNT(*) FILTER (WHERE created_at >= NOW() - INTERVAL '1 day')::BIGINT  AS d1,
            COUNT(*) FILTER (WHERE created_at >= NOW() - INTERVAL '7 days')::BIGINT AS d7,
            COUNT(*)::BIGINT                                                        AS d30
         FROM stats_events
         WHERE feature = $1 AND created_at >= NOW() - INTERVAL '30 days'
         GROUP BY action, status
         ORDER BY d30 DESC",
            &[&feature],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| ActionCount {
            action: r.get(0),
            status: r.get(1),
            d1: r.get(2),
            d7: r.get(3),
            d30: r.get(4),
        })
        .collect())
}

// ── error log ─────────────────────────────────────────────────────────────────

pub struct ErrorRow {
    pub feature: String,
    pub message: String,
    pub minutes_ago: i64,
}

// آخرین خطاهای ۲۴ ساعت گذشته (جدیدترین اول).
pub async fn get_recent_errors(
    client: &Client,
    limit: i64,
) -> Result<Vec<ErrorRow>, tokio_postgres::Error> {
    let rows = client
        .query(
            "SELECT feature, message,
            (EXTRACT(EPOCH FROM (NOW() - created_at)) / 60)::BIGINT AS minutes_ago
         FROM stats_errors
         WHERE created_at >= NOW() - INTERVAL '1 day'
         ORDER BY created_at DESC
         LIMIT $1",
            &[&limit],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| ErrorRow {
            feature: r.get(0),
            message: r.get(1),
            minutes_ago: r.get(2),
        })
        .collect())
}

// تعداد کل خطاهای ۲۴ ساعت گذشته (برای هدر).
pub async fn count_recent_errors(client: &Client) -> Result<i64, tokio_postgres::Error> {
    let row = client.query_one(
        "SELECT COUNT(*)::BIGINT FROM stats_errors WHERE created_at >= NOW() - INTERVAL '1 day'",
        &[],
    ).await?;
    Ok(row.get(0))
}

pub struct BroadcastCounts {
    pub total: i64,
    pub active: i64,
}

pub async fn get_broadcast_user_counts(
    client: &Client,
) -> Result<BroadcastCounts, tokio_postgres::Error> {
    let row = client
        .query_one(
            "SELECT
                COUNT(*)::BIGINT AS total,
                COUNT(*) FILTER (WHERE is_blocked = FALSE)::BIGINT AS active
             FROM stats_users",
            &[],
        )
        .await?;

    Ok(BroadcastCounts {
        total: row.get(0),
        active: row.get(1),
    })
}

pub async fn get_broadcast_user_ids(
    client: &Client,
    only_active: bool,
    limit: Option<i64>,
) -> Result<Vec<i64>, tokio_postgres::Error> {
    let rows = if only_active {
        if let Some(lim) = limit {
            client
                .query(
                    "SELECT user_id FROM stats_users WHERE is_blocked = FALSE ORDER BY last_seen DESC LIMIT $1",
                    &[&lim],
                )
                .await?
        } else {
            client
                .query(
                    "SELECT user_id FROM stats_users WHERE is_blocked = FALSE ORDER BY last_seen DESC",
                    &[],
                )
                .await?
        }
    } else if let Some(lim) = limit {
        client
            .query(
                "SELECT user_id FROM stats_users ORDER BY last_seen DESC LIMIT $1",
                &[&lim],
            )
            .await?
    } else {
        client
            .query("SELECT user_id FROM stats_users ORDER BY last_seen DESC", &[])
            .await?
    };

    Ok(rows.iter().map(|r| r.get::<_, i64>(0)).collect())
}

// ثانیه → نمایش فارسی فشرده برای پنل آمار (مثل "۱۲ ساعت" / "۴۵ دقیقه").
pub fn fmt_secs(total: i64) -> String {
    if total <= 0 {
        return "۰".to_string();
    }
    let h = total / 3600;
    let m = (total % 3600) / 60;
    if h > 0 {
        format!("{}h {}m", h, m)
    } else if m > 0 {
        format!("{}m", m)
    } else {
        format!("{}s", total)
    }
}

pub fn fmt_bytes(b: i64) -> String {
    const GB: i64 = 1 << 30;
    const MB: i64 = 1 << 20;
    if b >= GB {
        format!("{:.1} GB", b as f64 / GB as f64)
    } else {
        format!("{:.1} MB", b as f64 / MB as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmt_secs_zero_and_negative() {
        assert_eq!(fmt_secs(0), "۰");
        assert_eq!(fmt_secs(-5), "۰");
    }

    #[test]
    fn test_fmt_secs_hours_minutes_seconds() {
        assert_eq!(fmt_secs(45), "45s");
        assert_eq!(fmt_secs(120), "2m");
        assert_eq!(fmt_secs(3665), "1h 1m");
    }

    #[test]
    fn test_fmt_bytes_gb() {
        assert_eq!(fmt_bytes(2 * (1 << 30)), "2.0 GB");
    }

    #[test]
    fn test_fmt_bytes_mb() {
        assert_eq!(fmt_bytes(50 * (1 << 20)), "50.0 MB");
    }
}
