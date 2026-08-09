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

/// Spawn detached user-facing work.
///
/// `tokio::spawn` starts a fresh task, so it inherits neither the `LANG`
/// task-local (every `t()` inside would silently fall back to Persian) nor any
/// RAII guard held by the caller (the graceful-shutdown drain would not see the
/// work). This does both. Use it for anything that sends messages to a user;
/// use bare `tokio::spawn` for daemon tasks that must not delay shutdown.
pub fn spawn_user_task<F>(fut: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    // `try_with` must run here, on the caller's task — inside the spawn it is
    // always Err. This line is the whole fix; moving it in breaks it silently.
    let lang = crate::i18n::LANG
        .try_with(|l| l.clone())
        .unwrap_or_else(|_| "fa".to_owned());
    tokio::spawn(async move {
        let _guard = TaskGuard::new();
        crate::i18n::LANG.scope(lang, fut).await
    })
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
        }
        db
    } else {
        println!("DATABASE_URL is not set; cookie pool state is in-memory only.");
        None
    };

    let cookie_status = cookie_pool.status();

    let (rate_limit_tx, mut rate_limit_rx) = tokio::sync::mpsc::unbounded_channel();
    let (cooldown_done_tx, mut cooldown_done_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    fetch_bot_username(&api).await;
    spawn_cookie_refresher(&api, &mut cookie_pool);
    spawn_i18n_watcher();
    crate::ip_lookup::spawn_refresher();
    crate::feynobg::engine::spawn_session_reaper();
    crate::deoldify::engine::spawn_session_reaper();
    crate::moebius::spawn_session_reaper();
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

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
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

        // Signal shouldn't wait for current 30s long-poll to finish:
        // Whichever triggers first is used. `changed()` is cancel-safe and top condition breaks loop on next iteration.
        let updates = tokio::select! {
            biased;
            _ = shutdown_rx.changed() => continue,
            result = state.api.get_updates(&params) => match result {
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
            },
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

        while let Ok(source) = rate_limit_rx.try_recv() {
            spawn_cooldown_refresh(&state.api, source, cooldown_done_tx.clone());
        }

        for update in updates {
            params.offset = Some(update.update_id as i64 + 1);
            let _guard = TaskGuard::new();
            if let Err(e) = dispatch::handle_update(&mut state, update.content).await {
                let err_str = e.to_string();
                eprintln!("[main event=update_error] {err_str}");
                let ignored = err_str.contains("not enough rights")
                    || err_str.contains("Forbidden")
                    || err_str.contains("bot was blocked");
                if !ignored {
                    crate::stats::record_error_global("system", err_str).await;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering::SeqCst;

    /// Both properties of `spawn_user_task` tested together as `ACTIVE_TASKS`
    /// is global and tests run concurrently.
    #[tokio::test]
    async fn spawn_user_task_scopes_lang_and_holds_guard() {
        // 1) Language propagates from parent to spawned task.
        let inherited = crate::i18n::LANG
            .scope("en".to_owned(), async {
                spawn_user_task(async { crate::i18n::LANG.try_with(|l| l.clone()).ok() })
                    .await
                    .expect("spawned task must not panic")
            })
            .await;
        assert_eq!(inherited.as_deref(), Some("en"), "LANG did not propagate");

        // Rendered string inside task should be English, not Persian fallback.
        let rendered = crate::i18n::LANG
            .scope("en".to_owned(), async {
                spawn_user_task(async { crate::i18n::t("tts.cancel_button") })
                    .await
                    .expect("spawned task must not panic")
            })
            .await;
        assert_eq!(rendered, "❌ Cancel", "t() inside the task fell back to fa");

        // Counter-example: raw `tokio::spawn` renders in Persian fallback.
        let raw = crate::i18n::LANG
            .scope("en".to_owned(), async {
                tokio::spawn(async { crate::i18n::t("tts.cancel_button") })
                    .await
                    .expect("spawned task must not panic")
            })
            .await;
        assert_ne!(raw, rendered, "raw tokio::spawn unexpectedly kept LANG");

        // Without parent scope, fallback to "fa" applies.
        let scopeless = spawn_user_task(async { crate::i18n::LANG.try_with(|l| l.clone()).ok() })
            .await
            .expect("spawned task must not panic");
        assert_eq!(scopeless.as_deref(), Some("fa"));

        // 2) Task count maintained while running.
        let before = ACTIVE_TASKS.load(SeqCst);
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let handle = spawn_user_task(async move {
            let _ = started_tx.send(());
            let _ = release_rx.await;
        });
        let _ = started_rx.await;
        assert!(
            ACTIVE_TASKS.load(SeqCst) > before,
            "guard not held while the task runs; shutdown would not drain it"
        );
        let _ = release_tx.send(());
        handle.await.expect("spawned task must not panic");
        assert_eq!(
            ACTIVE_TASKS.load(SeqCst),
            before,
            "guard was never released"
        );
    }
}
