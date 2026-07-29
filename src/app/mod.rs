//! Core bot application runtime, update dispatcher loop, state management, and graceful shutdown.

#[cfg(feature = "testapi")]
pub mod dispatch;
#[cfg(not(feature = "testapi"))]
mod dispatch;
mod startup;
#[cfg(feature = "testapi")]
pub mod state;
#[cfg(not(feature = "testapi"))]
mod state;
use std::time::Duration;

use frankenstein::{AsyncTelegramApi, methods::GetUpdatesParams, types::AllowedUpdate};

use crate::config;
use crate::cookie_pool::CookiePool;
use crate::emoji::FlowManager;

use startup::{
    build_bot_api, fetch_bot_username, init_database, init_emoji_cache, set_bot_commands,
    spawn_cookie_refresher, spawn_cooldown_refresh, spawn_i18n_watcher, spawn_redeem_sweeper,
    spawn_referral_confirm_sweeper,
};
use state::AppState;

pub static ACTIVE_TASKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub struct TaskGuard;
impl TaskGuard {
    pub fn new() -> Self {
        ACTIVE_TASKS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self
    }
}
impl Drop for TaskGuard {
    fn drop(&mut self) {
        ACTIVE_TASKS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

pub async fn run() -> anyhow::Result<()> {
    crate::log::init_subscriber();
    let token = config::bot_token()?;
    let api = build_bot_api(&token).await?;
    let mut cookie_pool = CookiePool::from_default_firefox();

    let database = if let Some(database_url) = config::config_value("DATABASE_URL") {
        let db = init_database(&mut cookie_pool, &database_url).await;
        if db.is_some() {
            init_emoji_cache(&database_url).await;
            spawn_redeem_sweeper(&database_url);
            spawn_referral_confirm_sweeper(&database_url, &api);
        }
        db
    } else {
        println!("DATABASE_URL is not set; cookie pool state is in-memory only.");
        None
    };

    let cookie_status = cookie_pool.status();

    let (rate_limit_tx, mut rate_limit_rx) = tokio::sync::mpsc::unbounded_channel();
    let (flow_clear_tx, mut flow_clear_rx) = tokio::sync::mpsc::unbounded_channel::<i64>();
    let (cooldown_done_tx, mut cooldown_done_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    fetch_bot_username(&api).await;
    spawn_cookie_refresher(&api, &mut cookie_pool);
    spawn_i18n_watcher();
    crate::ip_lookup::spawn_refresher();
    set_bot_commands(&api).await;

    println!("Bot is running. Send /start to open the green button.");
    println!(
        "Cookie pool loaded: {} Firefox profile(s), {} selectable.",
        cookie_status.available_cookies, cookie_status.selectable_cookies
    );

    let mut state = AppState {
        api: api.clone(),
        cookie_pool: std::sync::Arc::new(tokio::sync::Mutex::new(cookie_pool)),
        database,
        flow_manager: FlowManager::new(),
        rate_limit_tx,
        flow_clear_tx,
        user_last_update: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
    };

    crate::health::mark_healthy();

    let health_port: u16 = config::config_value("HEALTH_PORT")
        .and_then(|p| p.parse().ok())
        .unwrap_or(14380);
    tokio::spawn(crate::health::serve(health_port));

    crate::health::mark_ready();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        let sigterm = async {
            #[cfg(unix)]
            {
                if let Ok(mut sig) =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                {
                    sig.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            }
            #[cfg(not(unix))]
            {
                std::future::pending::<()>().await;
            }
        };

        tokio::select! {
            _ = ctrl_c => tracing::info!(event = "signal_received", signal = "SIGINT"),
            _ = sigterm => tracing::info!(event = "signal_received", signal = "SIGTERM"),
        }
        let _ = shutdown_tx.send(true);
    });

    let mut params = GetUpdatesParams::builder()
        .timeout(30u32)
        .allowed_updates(vec![
            AllowedUpdate::Message,
            AllowedUpdate::CallbackQuery,
            AllowedUpdate::ChatMember,
        ])
        .build();

    loop {
        if *shutdown_rx.borrow() {
            tracing::info!(
                event = "shutdown_drain_start",
                "graceful shutdown initiated, draining tasks..."
            );
            let mut waited = 0;
            while ACTIVE_TASKS.load(std::sync::atomic::Ordering::SeqCst) > 0 && waited < 30 {
                tokio::time::sleep(Duration::from_secs(1)).await;
                waited += 1;
            }
            tracing::info!(
                event = "shutdown_complete",
                waited_secs = waited,
                "graceful shutdown complete"
            );
            break;
        }

        let updates = match state.api.get_updates(&params).await {
            Ok(response) => response.result,
            Err(error) => {
                let wait = match &error {
                    frankenstein::Error::Api(e) if e.error_code == 429 => {
                        e.parameters
                            .as_ref()
                            .and_then(|p| p.retry_after)
                            .unwrap_or(5)
                            .max(1) as u64
                    }
                    _ => 2,
                };
                eprintln!("get_updates failed: {error} (retry in {wait}s)");
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
        };

        while let Ok(cookie_id) = cooldown_done_rx.try_recv() {
            println!(
                "[cookie_refresher] cooldown refresh done, re-adding cookie_id={cookie_id} to pool"
            );
            state
                .cookie_pool
                .lock()
                .await
                .remove_from_cooldown(&cookie_id);
        }

        while let Ok(user_id) = flow_clear_rx.try_recv() {
            state.flow_manager.clear(user_id);
        }

        while let Ok(source) = rate_limit_rx.try_recv() {
            spawn_cooldown_refresh(&state.api, source, cooldown_done_tx.clone());
        }

        for update in updates {
            params.offset = Some(update.update_id as i64 + 1);
            let _guard = TaskGuard::new();
            if let Err(e) = dispatch::handle_update(&mut state, update.content).await {
                eprintln!("[main event=update_error] {e}");
                crate::stats::record_error_global("system", e).await;
            }
        }
    }
    Ok(())
}
