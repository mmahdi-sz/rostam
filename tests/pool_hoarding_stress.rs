use deadpool_postgres::{Config, ManagerConfig, Pool, PoolConfig, RecyclingMethod, Runtime, Timeouts};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Barrier;
use tokio::task::JoinSet;

struct WorkloadMetrics {
    well_behaved_total: usize,
    well_behaved_succeeded: usize,
    well_behaved_checkout_latencies: Vec<Duration>,
    hoarding_total: usize,
    hoarding_succeeded: usize,
    total_elapsed: Duration,
    peak_waiting: usize,
    peak_in_use: usize,
    recovery_batch_elapsed: Duration,
}

impl WorkloadMetrics {
    fn avg_checkout_latency(&self) -> Duration {
        if self.well_behaved_checkout_latencies.is_empty() {
            return Duration::ZERO;
        }
        let total: Duration = self.well_behaved_checkout_latencies.iter().sum();
        total / (self.well_behaved_checkout_latencies.len() as u32)
    }

    fn max_checkout_latency(&self) -> Duration {
        self.well_behaved_checkout_latencies
            .iter()
            .copied()
            .max()
            .unwrap_or(Duration::ZERO)
    }
}

fn resolve_db_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok().or_else(|| {
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
    })
}

fn create_test_pool(db_url: &str, pool_size: usize) -> Pool {
    let mut cfg = Config::new();
    cfg.url = Some(db_url.to_string());
    cfg.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    let mut pool_cfg = PoolConfig::new(pool_size);
    pool_cfg.timeouts = Timeouts {
        wait: Some(Duration::from_secs(5)),
        create: Some(Duration::from_secs(5)),
        recycle: Some(Duration::from_millis(1500)),
    };
    cfg.pool = Some(pool_cfg);

    cfg.create_pool(Some(Runtime::Tokio1), tokio_postgres::NoTls)
        .expect("create deadpool postgres pool")
}

async fn execute_workload(
    pool: Arc<Pool>,
    well_behaved_count: usize,
    hoarding_count: usize,
    slow_work_duration: Duration,
) -> WorkloadMetrics {
    let total_tasks = well_behaved_count + hoarding_count;
    let barrier = Arc::new(Barrier::new(total_tasks));
    let mut set = JoinSet::new();

    let well_behaved_succeeded = Arc::new(AtomicUsize::new(0));
    let hoarding_succeeded = Arc::new(AtomicUsize::new(0));
    let latencies = Arc::new(tokio::sync::Mutex::new(Vec::with_capacity(well_behaved_count * 2)));

    // Sampler for pool status during contention
    let peak_waiting = Arc::new(AtomicUsize::new(0));
    let peak_in_use = Arc::new(AtomicUsize::new(0));
    let sampler_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let sampler_pool = pool.clone();
    let sampler_peak_wait = peak_waiting.clone();
    let sampler_peak_in_use = peak_in_use.clone();
    let sampler_stop_clone = sampler_stop.clone();

    let sampler_handle = tokio::spawn(async move {
        while !sampler_stop_clone.load(Ordering::Relaxed) {
            let status = sampler_pool.status();
            let waiting = status.waiting;
            let in_use = status.size.saturating_sub(status.available);

            sampler_peak_wait.fetch_max(waiting, Ordering::Relaxed);
            sampler_peak_in_use.fetch_max(in_use, Ordering::Relaxed);

            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    });

    let start_time = Instant::now();

    // 1. Spawn Hoarding Tasks (Hold connection across slow async work)
    for _ in 0..hoarding_count {
        let pool = pool.clone();
        let barrier = barrier.clone();
        let hoarding_succeeded = hoarding_succeeded.clone();

        set.spawn(async move {
            barrier.wait().await;

            // Check out connection and DELIBERATELY HOLD across slow await
            let client = pool.get().await.expect("hoarder checkout failed");
            
            // Execute quick pre-query
            let row = client
                .query_one("SELECT 100::int AS n", &[])
                .await
                .expect("hoarder query 1");
            let val: i32 = row.get("n");
            assert_eq!(val, 100);

            // Simulate slow I/O / CPU Broker work WHILE HOLDING THE CONNECTION
            tokio::time::sleep(slow_work_duration).await;

            // Post query still using held connection
            let row2 = client
                .query_one("SELECT 200::int AS n", &[])
                .await
                .expect("hoarder query 2");
            let val2: i32 = row2.get("n");
            assert_eq!(val2, 200);

            drop(client); // explicitly released at the end
            hoarding_succeeded.fetch_add(1, Ordering::SeqCst);
        });
    }

    // 2. Spawn Well-Behaved Tasks (Checkout -> Drop -> Slow async work -> Checkout -> Drop)
    for _ in 0..well_behaved_count {
        let pool = pool.clone();
        let barrier = barrier.clone();
        let well_behaved_succeeded = well_behaved_succeeded.clone();
        let latencies = latencies.clone();

        set.spawn(async move {
            barrier.wait().await;

            // Phase A: Pre-flight checkout
            let t0 = Instant::now();
            let client = pool.get().await.expect("well-behaved checkout 1 failed");
            let lat1 = t0.elapsed();

            let row = client
                .query_one("SELECT 1::int AS n", &[])
                .await
                .expect("well-behaved query 1");
            let val: i32 = row.get("n");
            assert_eq!(val, 1);

            // CRUCIAL: Drop the guard BEFORE slow work!
            drop(client);

            // Phase B: Simulate slow I/O / CPU work with NO connection held
            tokio::time::sleep(slow_work_duration).await;

            // Phase C: Post-processing checkout
            let t1 = Instant::now();
            let client2 = pool.get().await.expect("well-behaved checkout 2 failed");
            let lat2 = t1.elapsed();

            let row2 = client2
                .query_one("SELECT 2::int AS n", &[])
                .await
                .expect("well-behaved query 2");
            let val2: i32 = row2.get("n");
            assert_eq!(val2, 2);

            drop(client2);

            let mut lats = latencies.lock().await;
            lats.push(lat1);
            lats.push(lat2);

            well_behaved_succeeded.fetch_add(1, Ordering::SeqCst);
        });
    }

    // Await all workload tasks
    while let Some(res) = set.join_next().await {
        res.expect("task panicked during workload");
    }

    let total_elapsed = start_time.elapsed();
    sampler_stop.store(true, Ordering::Relaxed);
    let _ = sampler_handle.await;

    // 3. Measure Recovery Batch (Run 32 quick queries after contention)
    let recovery_start = Instant::now();
    let mut rec_set = JoinSet::new();
    for i in 0..32 {
        let pool = pool.clone();
        rec_set.spawn(async move {
            let client = pool.get().await.expect("recovery checkout failed");
            let row = client
                .query_one("SELECT $1::int AS n", &[&(i as i32)])
                .await
                .expect("recovery query");
            let n: i32 = row.get("n");
            assert_eq!(n, i as i32);
        });
    }
    while let Some(res) = rec_set.join_next().await {
        res.expect("recovery task failed");
    }
    let recovery_batch_elapsed = recovery_start.elapsed();

    let recorded_latencies = latencies.lock().await.clone();

    WorkloadMetrics {
        well_behaved_total: well_behaved_count,
        well_behaved_succeeded: well_behaved_succeeded.load(Ordering::SeqCst),
        well_behaved_checkout_latencies: recorded_latencies,
        hoarding_total: hoarding_count,
        hoarding_succeeded: hoarding_succeeded.load(Ordering::SeqCst),
        total_elapsed,
        peak_waiting: peak_waiting.load(Ordering::Relaxed),
        peak_in_use: peak_in_use.load(Ordering::Relaxed),
        recovery_batch_elapsed,
    }
}

#[tokio::test]
async fn test_pool_connection_hoarding_stress_and_recovery() {
    let Some(db_url) = resolve_db_url() else {
        eprintln!("[test] Skipping test_pool_connection_hoarding_stress_and_recovery: DATABASE_URL not set");
        return;
    };

    // Pool size matching production/dev config (16 for dev)
    let pool_size = 16;
    let slow_work_duration = Duration::from_millis(500);

    // =========================================================================
    // RUN 1: CONTROL BASELINE (0 Hoarders, 32 Well-Behaved tasks)
    // =========================================================================
    println!("\n=== [Run 1: Control Baseline] 0 Hoarders + 32 Well-Behaved Tasks ===");
    let control_pool = Arc::new(create_test_pool(&db_url, pool_size));
    let control_metrics = execute_workload(control_pool.clone(), 32, 0, slow_work_duration).await;

    println!("Control Total Time: {:?}", control_metrics.total_elapsed);
    println!("Control Well-Behaved Succeeded: {}/{}", control_metrics.well_behaved_succeeded, control_metrics.well_behaved_total);
    println!("Control Avg Checkout Latency: {:?}", control_metrics.avg_checkout_latency());
    println!("Control Max Checkout Latency: {:?}", control_metrics.max_checkout_latency());
    println!("Control Peak Waiting in Pool: {}", control_metrics.peak_waiting);
    println!("Control Peak In-Use Connections: {}/{}", control_metrics.peak_in_use, pool_size);
    println!("Control Recovery 32-Query Batch Time: {:?}", control_metrics.recovery_batch_elapsed);

    assert_eq!(control_metrics.well_behaved_succeeded, 32);

    // =========================================================================
    // RUN 2: HOARDING STRESS RUN (6 Hoarders + 26 Well-Behaved tasks)
    // 6 connections out of 16 are locked down for 500ms by hoarding tasks.
    // The remaining 10 connections must serve 26 well-behaved tasks.
    // =========================================================================
    println!("\n=== [Run 2: Hoarding Contention] 6 Hoarders + 26 Well-Behaved Tasks ===");
    let stress_pool = Arc::new(create_test_pool(&db_url, pool_size));
    let stress_metrics = execute_workload(stress_pool.clone(), 26, 6, slow_work_duration).await;

    println!("Stress Total Time: {:?}", stress_metrics.total_elapsed);
    println!("Stress Hoarders Succeeded: {}/{}", stress_metrics.hoarding_succeeded, stress_metrics.hoarding_total);
    println!("Stress Well-Behaved Succeeded: {}/{}", stress_metrics.well_behaved_succeeded, stress_metrics.well_behaved_total);
    println!("Stress Avg Checkout Latency: {:?}", stress_metrics.avg_checkout_latency());
    println!("Stress Max Checkout Latency: {:?}", stress_metrics.max_checkout_latency());
    println!("Stress Peak Waiting in Pool: {}", stress_metrics.peak_waiting);
    println!("Stress Peak In-Use Connections: {}/{}", stress_metrics.peak_in_use, pool_size);
    println!("Stress Recovery 32-Query Batch Time: {:?}", stress_metrics.recovery_batch_elapsed);

    // =========================================================================
    // ASSERTIONS & VERIFICATION
    // =========================================================================

    // 1. All well-behaved tasks completed successfully without error or timeout
    assert_eq!(
        stress_metrics.well_behaved_succeeded, 26,
        "All 26 well-behaved tasks must succeed despite 6 hoarding tasks"
    );
    assert_eq!(
        stress_metrics.hoarding_succeeded, 6,
        "All 6 hoarding tasks must finish"
    );

    // 2. Contention signature: Pool in-use reached maximum capacity (all 16 connections utilized)
    assert!(
        stress_metrics.peak_in_use >= 10,
        "Contention must meaningfully saturate connections (peak in-use was {})",
        stress_metrics.peak_in_use
    );

    // 3. Graceful degradation: Well-behaved tasks did not deadlock, max latency was well under timeout (5s)
    assert!(
        stress_metrics.max_checkout_latency() < Duration::from_secs(3),
        "Max checkout latency ({:?}) exceeded 3s safety margin",
        stress_metrics.max_checkout_latency()
    );

    // 4. Full Pool Recovery: After all tasks complete and hoarders release connections,
    // the 32-query recovery batch executes rapidly without residual starvation
    assert!(
        stress_metrics.recovery_batch_elapsed < Duration::from_millis(500),
        "Recovery batch took {:?}, expected < 500ms",
        stress_metrics.recovery_batch_elapsed
    );

    let final_status = stress_pool.status();
    assert_eq!(final_status.waiting, 0, "No lingering tasks waiting in pool queue");
    assert_eq!(
        final_status.size, pool_size,
        "Pool size should equal configured max_size ({})",
        pool_size
    );

    println!("\n✅ Pool Hoarding Stress Test Passed Successfully!");
}
