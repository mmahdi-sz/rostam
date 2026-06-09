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
    let row = client.query_one(
        "SELECT
            COUNT(*)                                                      AS total,
            COUNT(*) FILTER (WHERE first_seen >= NOW() - INTERVAL '1 day')   AS new_1d,
            COUNT(*) FILTER (WHERE first_seen >= NOW() - INTERVAL '3 days')  AS new_3d,
            COUNT(*) FILTER (WHERE first_seen >= NOW() - INTERVAL '7 days')  AS new_7d,
            COUNT(*) FILTER (WHERE first_seen >= NOW() - INTERVAL '30 days') AS new_30d
         FROM stats_users",
        &[],
    ).await?;

    Ok(UserStats {
        total:   row.get::<_, i64>(0),
        new_1d:  row.get::<_, i64>(1),
        new_3d:  row.get::<_, i64>(2),
        new_7d:  row.get::<_, i64>(3),
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
        requests_1d:  row.get::<_, i64>(0),
        requests_3d:  row.get::<_, i64>(1),
        requests_7d:  row.get::<_, i64>(2),
        requests_30d: row.get::<_, i64>(3),

        bytes_downloaded_1d:  row.get::<_, i64>(4),
        bytes_downloaded_3d:  row.get::<_, i64>(5),
        bytes_downloaded_7d:  row.get::<_, i64>(6),
        bytes_downloaded_30d: row.get::<_, i64>(7),

        uploads_ok_1d:  row.get::<_, i64>(8),
        uploads_ok_3d:  row.get::<_, i64>(9),
        uploads_ok_7d:  row.get::<_, i64>(10),
        uploads_ok_30d: row.get::<_, i64>(11),

        bytes_uploaded_1d:  row.get::<_, i64>(12),
        bytes_uploaded_3d:  row.get::<_, i64>(13),
        bytes_uploaded_7d:  row.get::<_, i64>(14),
        bytes_uploaded_30d: row.get::<_, i64>(15),
    })
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
