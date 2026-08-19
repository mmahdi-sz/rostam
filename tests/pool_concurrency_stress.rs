use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;

#[tokio::test]
async fn test_pool_concurrent_checkout_and_recycling() {
    let db_url = std::env::var("DATABASE_URL").ok().or_else(|| {
        std::fs::read_to_string(".env").ok().and_then(|content| {
            content.lines().find_map(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with("DATABASE_URL=") {
                    Some(trimmed.strip_prefix("DATABASE_URL=")?.trim_matches('"').to_string())
                } else {
                    None
                }
            })
        })
    });
    let Some(db_url) = db_url else {
        eprintln!("[test] Skipping test_pool_concurrent_checkout_and_recycling: DATABASE_URL not found");
        return;
    };

    let pool_size = 16;
    let mut cfg = deadpool_postgres::Config::new();
    cfg.url = Some(db_url);
    cfg.manager = Some(deadpool_postgres::ManagerConfig {
        recycling_method: deadpool_postgres::RecyclingMethod::Fast,
    });
    let mut pool_cfg = deadpool_postgres::PoolConfig::new(pool_size);
    pool_cfg.timeouts = deadpool_postgres::Timeouts {
        wait: Some(Duration::from_millis(3000)),
        create: Some(Duration::from_millis(3000)),
        recycle: Some(Duration::from_millis(1000)),
    };
    cfg.pool = Some(pool_cfg);

    let pool = cfg
        .create_pool(Some(deadpool_postgres::Runtime::Tokio1), tokio_postgres::NoTls)
        .expect("create pool");

    // Spawn 64 concurrent tasks executing queries against the pool of max size 16
    let task_count = 64;
    let mut set = JoinSet::new();
    let pool = Arc::new(pool);

    for i in 0..task_count {
        let pool = pool.clone();
        set.spawn(async move {
            let client = pool.get().await.expect("checkout client from pool");
            let row = client
                .query_one("SELECT $1::int AS n", &[&(i as i32)])
                .await
                .expect("query execution");
            let n: i32 = row.get("n");
            assert_eq!(n, i as i32);
            // Simulate short work
            tokio::time::sleep(Duration::from_millis(10)).await;
            // Connection is returned to pool upon drop
        });
    }

    let mut completed = 0;
    while let Some(res) = set.join_next().await {
        res.expect("task join error");
        completed += 1;
    }

    assert_eq!(completed, task_count);

    // Verify pool status after all tasks finished
    let status = pool.status();
    assert_eq!(status.waiting, 0, "No pending tasks waiting for connection");
    assert!(
        status.available <= pool_size,
        "Available connections within pool bound"
    );
}
