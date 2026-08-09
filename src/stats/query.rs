use std::collections::HashMap;
use tokio_postgres::Client;

/// `stats_events` features that are not user-facing tools — excluded from
/// "top feature", which otherwise reports `paywall` (a block, not a use).
pub const NON_FEATURE_EVENTS: &[&str] = &["paywall", "cpu", "cookie", "broadcast", "referral"];

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

/// Four standard periods for stats panel.
pub struct Periods {
    pub d1: i64,
    /// Kept for the users/downloads panels; feature blocks only print 1d/7d/30d.
    #[allow(dead_code)]
    pub d3: i64,
    pub d7: i64,
    pub d30: i64,
}

pub struct FeatureStats {
    pub ok: Periods,     // Successful events
    pub fail: Periods,   // Failed events (fail/timeout/etc.)
    pub amount: Periods, // Total amount for successful events (seconds or count depending on feature)
}

/// Per-feature stats from `stats_events`, batched into one round-trip.
/// Features with no events are absent from the map.
pub async fn get_feature_stats_multi(
    client: &Client,
    features: &[&str],
) -> Result<HashMap<String, FeatureStats>, tokio_postgres::Error> {
    let list: Vec<String> = features.iter().map(|f| (*f).to_string()).collect();
    let rows = client.query(
        "SELECT feature,
            COUNT(*) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '1 day')::BIGINT   AS ok_1d,
            COUNT(*) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '3 days')::BIGINT  AS ok_3d,
            COUNT(*) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '7 days')::BIGINT  AS ok_7d,
            COUNT(*) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '30 days')::BIGINT AS ok_30d,

            COUNT(*) FILTER (WHERE status <> 'ok' AND created_at >= NOW() - INTERVAL '1 day')::BIGINT   AS fail_1d,
            COUNT(*) FILTER (WHERE status <> 'ok' AND created_at >= NOW() - INTERVAL '3 days')::BIGINT  AS fail_3d,
            COUNT(*) FILTER (WHERE status <> 'ok' AND created_at >= NOW() - INTERVAL '7 days')::BIGINT  AS fail_7d,
            COUNT(*) FILTER (WHERE status <> 'ok' AND created_at >= NOW() - INTERVAL '30 days')::BIGINT AS fail_30d,

            COALESCE(SUM(amount) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '1 day'),   0)::BIGINT AS amt_1d,
            COALESCE(SUM(amount) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '3 days'),  0)::BIGINT AS amt_3d,
            COALESCE(SUM(amount) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '7 days'),  0)::BIGINT AS amt_7d,
            COALESCE(SUM(amount) FILTER (WHERE status = 'ok' AND created_at >= NOW() - INTERVAL '30 days'), 0)::BIGINT AS amt_30d
         FROM stats_events
         WHERE feature = ANY($1) AND created_at >= NOW() - INTERVAL '30 days'
         GROUP BY feature",
        &[&list],
    ).await?;

    Ok(rows
        .iter()
        .map(|r| {
            (
                r.get::<_, String>(0),
                FeatureStats {
                    ok: Periods {
                        d1: r.get(1),
                        d3: r.get(2),
                        d7: r.get(3),
                        d30: r.get(4),
                    },
                    fail: Periods {
                        d1: r.get(5),
                        d3: r.get(6),
                        d7: r.get(7),
                        d30: r.get(8),
                    },
                    amount: Periods {
                        d1: r.get(9),
                        d3: r.get(10),
                        d7: r.get(11),
                        d30: r.get(12),
                    },
                },
            )
        })
        .collect())
}

// ── active users ────────────────────────────────────────────────────────────────

pub struct ActiveUsers {
    pub dau: i64,            // Active in past 24 hours (last_seen)
    pub wau: i64,            // Active in past 7 days
    pub returning_1d: i64,   // Returning active users today (first_seen > 1 day ago)
    pub top_feature: String, // Top AI feature in 7 days (raw key, empty if none)
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

    let excluded: Vec<String> = NON_FEATURE_EVENTS
        .iter()
        .map(|f| (*f).to_string())
        .collect();
    let top = client
        .query_opt(
            "SELECT feature, COUNT(*)::BIGINT AS c
         FROM stats_events
         WHERE created_at >= NOW() - INTERVAL '7 days'
           AND feature <> ALL($1)
         GROUP BY feature
         ORDER BY c DESC
         LIMIT 1",
            &[&excluded],
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

// ── action/status breakdown ──────────────────────────────────────────────────

pub struct ActionCount {
    pub action: String,
    pub status: String,
    pub d1: i64,
    pub d7: i64,
    pub d30: i64,
}

/// Breakdown by (action, status) over 30 days for many features, one round-trip.
/// Rows are ordered descending by the 30-day count.
pub async fn get_action_breakdown_multi(
    client: &Client,
    features: &[&str],
) -> Result<HashMap<String, Vec<ActionCount>>, tokio_postgres::Error> {
    let list: Vec<String> = features.iter().map(|f| (*f).to_string()).collect();
    let rows = client
        .query(
            "SELECT feature, action, status,
            COUNT(*) FILTER (WHERE created_at >= NOW() - INTERVAL '1 day')::BIGINT  AS d1,
            COUNT(*) FILTER (WHERE created_at >= NOW() - INTERVAL '7 days')::BIGINT AS d7,
            COUNT(*)::BIGINT                                                        AS d30
         FROM stats_events
         WHERE feature = ANY($1) AND created_at >= NOW() - INTERVAL '30 days'
         GROUP BY feature, action, status
         ORDER BY d30 DESC",
            &[&list],
        )
        .await?;

    let mut map: HashMap<String, Vec<ActionCount>> = HashMap::new();
    for r in rows.iter() {
        map.entry(r.get(0)).or_default().push(ActionCount {
            action: r.get(1),
            status: r.get(2),
            d1: r.get(3),
            d7: r.get(4),
            d30: r.get(5),
        });
    }
    Ok(map)
}

// ── error log ─────────────────────────────────────────────────────────────────
pub struct ErrorRow {
    pub feature: String,
    pub message: String,
    pub minutes_ago: i64,
}

/// Recent errors in past 24 hours (newest first).
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

/// Total errors in past 24 hours.
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
            .query(
                "SELECT user_id FROM stats_users ORDER BY last_seen DESC",
                &[],
            )
            .await?
    };

    Ok(rows.iter().map(|r| r.get::<_, i64>(0)).collect())
}

/// Formats seconds into compact string for stats panel (e.g. "12h 0m", "45m", "30s").
pub fn fmt_secs(total: i64) -> String {
    if total <= 0 {
        return "0".to_string();
    }
    let h = total / 3600;
    let m = (total % 3600) / 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{total}s")
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
        // Stats panel keeps English digits.
        assert_eq!(fmt_secs(0), "0");
        assert_eq!(fmt_secs(-5), "0");
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
