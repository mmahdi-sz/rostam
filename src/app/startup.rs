use std::sync::Arc;
use std::time::Duration;

use frankenstein::client_reqwest::Bot;
use frankenstein::{
    AsyncTelegramApi,
    methods::{SetChatMenuButtonParams, SetMyCommandsParams},
    types::{BotCommand, MenuButton},
};
use tokio::sync::RwLock;

use crate::config;
use crate::cookie_pool::{CookiePool, CookieSource};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji;
use crate::i18n::reload_i18n;
use crate::modules;

pub async fn build_bot_api(token: &str) -> Result<Bot, Box<dyn std::error::Error>> {
    let Some(base_url) = config::bot_api_base_url() else {
        println!("BOT_API_BASE_URL is not set; using official Telegram Bot API.");
        return Ok(Bot::new(token));
    };
    let base_url = base_url.trim_end_matches('/').to_string();
    if base_url.contains("127.0.0.1") || base_url.contains("localhost") {
        println!("Local Bot API base detected ({base_url}); logging out from official Telegram Bot API.");
        let official_api = Bot::new(token);
        match official_api.log_out().await {
            Ok(response) => println!("Official Telegram Bot API logOut result: {}", response.result),
            Err(error) => {
                let desc = error.to_string();
                if desc.contains("Logged out") || desc.contains("Unauthorized") {
                    println!("Already logged out from official Telegram Bot API; continuing.");
                } else {
                    return Err(error.into());
                }
            }
        }
    } else {
        println!("Custom Bot API base detected ({base_url}); skipping automatic official logOut.");
    }
    println!("Bot API client initialized with base: {base_url}/bot<token>");
    Ok(Bot::new_url(format!("{base_url}/bot{token}")))
}

pub async fn init_database(
    cookie_pool: &mut CookiePool,
    database_url: &str,
) -> Option<PostgresDatabase> {
    match PostgresDatabase::connect(database_url).await {
        Ok(database) => {
            match database.load_state().await {
                Ok((last_used_cookie, cooldown_list)) => {
                    cookie_pool.restore_state(last_used_cookie, cooldown_list);
                }
                Err(e) => eprintln!("failed to load cookie pool state: {e}"),
            }
            if let Err(e) = database.save_snapshot(&cookie_pool.snapshot()).await {
                eprintln!("failed to save cookie pool snapshot: {e}");
            }
            println!("PostgreSQL cookie pool storage is enabled.");
            Some(database)
        }
        Err(e) => {
            eprintln!("failed to connect to PostgreSQL: {e}");
            None
        }
    }
}

pub async fn init_emoji_cache(database_url: &str) {
    let Some(admin_id) = config::admin_user_id() else {
        println!("ADMIN_USER_ID not set; emoji cache disabled.");
        return;
    };
    let Ok((client, conn)) = tokio_postgres::connect(database_url, tokio_postgres::NoTls).await else {
        eprintln!("emoji cache: failed initial DB connection");
        return;
    };
    tokio::spawn(conn);

    let initial = emoji::cache::load_from_db(&client, admin_id).await;
    let cache_arc = Arc::new(RwLock::new(initial));
    let _ = emoji::cache::CACHE.set(cache_arc.clone());
    println!("Emoji cache loaded for admin user {admin_id}.");

    let db_url = database_url.to_string();
    tokio::spawn(async move {
        let Ok((refresh_client, refresh_conn)) = tokio_postgres::connect(&db_url, tokio_postgres::NoTls).await else {
            eprintln!("emoji cache refresh: failed to connect");
            return;
        };
        tokio::spawn(refresh_conn);
        loop {
            tokio::time::sleep(Duration::from_secs(300)).await;
            let fresh = emoji::cache::load_from_db(&refresh_client, admin_id).await;
            *cache_arc.write().await = fresh;
            println!("Emoji cache refreshed.");
        }
    });
}

pub fn spawn_cookie_refresher(api: &Bot, cookie_pool: &mut CookiePool) {
    let profiles: Vec<(String, String, String)> = cookie_pool
        .snapshot()
        .available_cookies
        .into_iter()
        .map(|c| (
            c.profile_name,
            c.source_profile_dir.to_string_lossy().into_owned(),
            c.profile_dir.to_string_lossy().into_owned(),
        ))
        .collect();

    if profiles.is_empty() {
        println!("[cookie_refresher] no profiles found, skipping.");
        return;
    }

    let admin_chat_id = config::admin_user_id().unwrap_or(0);
    let refresh_interval_secs: u64 = config::config_value("COOKIE_REFRESH_INTERVAL_SECS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(6 * 3600);
    let refresh_api = api.clone();

    println!(
        "[cookie_refresher] background task starting: {} profile(s), interval={}s",
        profiles.len(), refresh_interval_secs
    );

    tokio::spawn(async move {
        loop {
            for chunk in profiles.chunks(3) {
                println!(
                    "[cookie_refresher] starting chunk of {} profile(s): {}",
                    chunk.len(),
                    chunk.iter().map(|(n, _, _)| n.as_str()).collect::<Vec<_>>().join(", ")
                );
                let futs: Vec<_> = chunk.iter().map(|(profile_name, profile_path, cache_dir)| {
                    let cfg = modules::cookie_refresher::CookieRefresherConfig {
                        profile_path: profile_path.clone(),
                        profile_name: profile_name.clone(),
                        cache_dir: cache_dir.clone(),
                        links_file: "files/youtube_links.txt".to_string(),
                        duration_secs: 600,
                        link_count: 1,
                        admin_chat_id,
                    };
                    let api = refresh_api.clone();
                    let pname = profile_name.clone();
                    async move {
                        println!("[cookie_refresher] running for profile={pname}");
                        if let Err(e) = modules::cookie_refresher::run(&api, cfg).await {
                            eprintln!("[cookie_refresher] profile={pname} error: {e}");
                        } else {
                            println!("[cookie_refresher] profile={pname} cookies updated on disk");
                        }
                    }
                }).collect();
                futures::future::join_all(futs).await;
                println!("[cookie_refresher] chunk done");
            }
            println!("[cookie_refresher] all profiles done, sleeping {refresh_interval_secs}s");
            tokio::time::sleep(Duration::from_secs(refresh_interval_secs)).await;
        }
    });
}

pub fn spawn_cooldown_refresh(
    api: &Bot,
    source: CookieSource,
    done_tx: tokio::sync::mpsc::UnboundedSender<String>,
) {
    let api_clone = api.clone();
    let admin_chat_id = config::admin_user_id().unwrap_or(0);
    tokio::spawn(async move {
        let p = &source.profile_name;
        let cookie_id = source.id.clone();
        tokio::time::sleep(Duration::from_secs(30 * 60)).await;
        println!("[cookie_refresh profile={p} event=cooldown_refresh_start] cookie_id={cookie_id}");
        let cfg = modules::cookie_refresher::CookieRefresherConfig {
            profile_path: source.source_profile_dir.to_string_lossy().into_owned(),
            profile_name: source.profile_name.clone(),
            cache_dir: source.profile_dir.to_string_lossy().into_owned(),
            links_file: "files/youtube_links.txt".to_string(),
            duration_secs: 600,
            link_count: 1,
            admin_chat_id,
        };
        if let Err(e) = modules::cookie_refresher::run(&api_clone, cfg).await {
            eprintln!("[cookie_refresh profile={p} event=cooldown_refresh_failed] cookie_id={cookie_id} err={e}");
        } else {
            println!("[cookie_refresh profile={p} event=cooldown_refresh_done] cookie_id={cookie_id}");
        }
        let _ = done_tx.send(cookie_id);
    });
}

pub fn spawn_i18n_watcher() {
    tokio::task::spawn_blocking(|| {
        use notify::{EventKind, RecursiveMode, Watcher, recommended_watcher};
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = recommended_watcher(tx).expect("failed to create file watcher");
        watcher
            .watch(std::path::Path::new("i18n.json"), RecursiveMode::NonRecursive)
            .expect("failed to watch i18n.json");
        eprintln!("[i18n] watching i18n.json for changes");
        for res in rx {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Modify(_) | EventKind::Create(_) => reload_i18n(),
                    _ => {}
                }
            }
        }
    });
}

pub async fn fetch_bot_username(api: &Bot) {
    match api.get_me().await {
        Ok(resp) => {
            let username = resp.result.username.unwrap_or_default();
            println!("Bot username: @{username}");
            crate::config::set_bot_username(username);
        }
        Err(e) => eprintln!("Failed to fetch bot username: {e}"),
    }
}

pub async fn set_bot_commands(api: &Bot) {
    let menu_params = SetChatMenuButtonParams::builder()
        .menu_button(MenuButton::Commands)
        .build();
    match api.set_chat_menu_button(&menu_params).await {
        Ok(_) => println!("Chat menu button set to Commands."),
        Err(e) => eprintln!("Failed to set chat menu button: {e}"),
    }

    let commands = vec![
        BotCommand { command: "start".to_string(), description: "منوی اصلی".to_string() },
        BotCommand { command: "emoji".to_string(), description: "پنل مدیریت ایموجی".to_string() },
        BotCommand { command: "se".to_string(), description: "تنظیم لقب ایموجی".to_string() },
    ];
    match api.set_my_commands(&SetMyCommandsParams::builder().commands(commands).build()).await {
        Ok(_) => println!("Bot commands set successfully."),
        Err(e) => eprintln!("Failed to set bot commands: {e}"),
    }
}
