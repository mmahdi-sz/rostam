mod dispatch;
mod startup;
mod state;

use std::time::Duration;

use frankenstein::{AsyncTelegramApi, methods::GetUpdatesParams};

use crate::config;
use crate::cookie_pool::CookiePool;
use crate::emoji::FlowManager;

use startup::{
    build_bot_api, init_database, init_emoji_cache,
    set_bot_commands, spawn_cookie_refresher, spawn_cooldown_refresh, spawn_i18n_watcher,
};
use state::AppState;

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let token = config::bot_token()?;
    let api = build_bot_api(&token).await?;
    let mut cookie_pool = CookiePool::from_default_firefox();

    let database = if let Some(database_url) = config::config_value("DATABASE_URL") {
        let db = init_database(&mut cookie_pool, &database_url).await;
        if db.is_some() {
            init_emoji_cache(&database_url).await;
        }
        db
    } else {
        println!("DATABASE_URL is not set; cookie pool state is in-memory only.");
        None
    };

    let cookie_status = cookie_pool.status();

    let (rate_limit_tx, mut rate_limit_rx) = tokio::sync::mpsc::unbounded_channel();
    let (cooldown_done_tx, mut cooldown_done_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    spawn_cookie_refresher(&api, &mut cookie_pool);
    spawn_i18n_watcher();
    set_bot_commands(&api).await;

    println!("Bot is running. Send /start to open the green button.");
    println!(
        "Cookie pool loaded: {} Firefox profile(s), {} selectable.",
        cookie_status.available_cookies, cookie_status.selectable_cookies
    );

    let mut state = AppState {
        api: api.clone(),
        cookie_pool,
        database,
        flow_manager: FlowManager::new(),
        rate_limit_tx,
    };

    let mut params = GetUpdatesParams::builder().timeout(30u32).build();

    loop {
        let updates = match state.api.get_updates(&params).await {
            Ok(response) => response.result,
            Err(error) => {
                eprintln!("get_updates failed: {error}");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        while let Ok(cookie_id) = cooldown_done_rx.try_recv() {
            println!("[cookie_refresher] cooldown refresh done, re-adding cookie_id={cookie_id} to pool");
            state.cookie_pool.remove_from_cooldown(&cookie_id);
        }

        while let Ok(source) = rate_limit_rx.try_recv() {
            spawn_cooldown_refresh(&state.api, source, cooldown_done_tx.clone());
        }

        for update in updates {
            params.offset = Some(update.update_id as i64 + 1);
            if let Err(e) = dispatch::handle_update(&mut state, update.content).await {
                eprintln!("[main event=update_error] {e}");
            }
        }
    }
}
