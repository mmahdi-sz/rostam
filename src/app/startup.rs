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
use crate::i18n::{LANG, reload_i18n, t};
use crate::modules;
use crate::stats;

/// Sentinel marking that this token has already been migrated off the official
/// Bot API. `logOut` is a one-time migration step — Telegram rate-limits it, and
/// with `Restart=always` a rate-limited retry loop feeds itself.
const LOGOUT_SENTINEL: &str = "files/.official_logout_done";

pub async fn build_bot_api(token: &str) -> anyhow::Result<Bot> {
    let Some(base_url) = config::bot_api_base_url() else {
        println!("BOT_API_BASE_URL is not set; using official Telegram Bot API.");
        return Ok(Bot::new(token));
    };
    let base_url = base_url.trim_end_matches('/').to_string();
    if base_url.contains("127.0.0.1") || base_url.contains("localhost") {
        if std::path::Path::new(LOGOUT_SENTINEL).exists() {
            println!("Already migrated to the local Bot API ({LOGOUT_SENTINEL}); skipping logOut.");
        } else {
            println!(
                "Local Bot API base detected ({base_url}); logging out from official Telegram Bot API."
            );
            let official_api = Bot::new(token);
            match official_api.log_out().await {
                Ok(response) => println!(
                    "Official Telegram Bot API logOut result: {}",
                    response.result
                ),
                Err(error) => {
                    let desc = error.to_string();
                    if desc.contains("Logged out") || desc.contains("Unauthorized") {
                        println!("Already logged out from official Telegram Bot API; continuing.");
                    } else {
                        // Never fail startup here: with Restart=always a failed
                        // logOut restart-loops and every loop sends another
                        // logOut, driving the rate limit deeper. If the token
                        // really is still bound to the official API, getUpdates
                        // against the local server reports it.
                        eprintln!(
                            "official logOut failed ({desc}); continuing with the local Bot API."
                        );
                    }
                }
            }
            if let Some(parent) = std::path::Path::new(LOGOUT_SENTINEL).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(LOGOUT_SENTINEL, "") {
                eprintln!(
                    "could not write {LOGOUT_SENTINEL} ({e}); logOut will run again on the next restart"
                );
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
            // init stats with client
            let client_ref: &'static tokio_postgres::Client =
                unsafe { &*(database.client() as *const _) };
            stats::init(client_ref);
            crate::rank::prices::load();
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
    let Ok((client, conn)) = tokio_postgres::connect(database_url, tokio_postgres::NoTls).await
    else {
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
        let Ok((refresh_client, refresh_conn)) =
            tokio_postgres::connect(&db_url, tokio_postgres::NoTls).await
        else {
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

/// Periodic cleanup of expired gift codes (7-day sliding ttl). Runs hourly.
pub fn spawn_redeem_sweeper(database_url: &str) {
    let db_url = database_url.to_string();
    tokio::spawn(async move {
        let Ok((client, conn)) = tokio_postgres::connect(&db_url, tokio_postgres::NoTls).await
        else {
            eprintln!("[redeem event=sweeper_connect_failed]");
            crate::stats::record_error_global("redeem_sweeper", "DB connect failed").await;
            return;
        };
        tokio::spawn(conn);
        loop {
            match crate::redeem::store::sweep_expired(&client).await {
                Ok(n) if n > 0 => eprintln!("[redeem event=sweep_done removed={n}]"),
                Ok(_) => {}
                Err(e) => {
                    eprintln!("[redeem event=sweep_failed err={e}]");
                    crate::stats::record_error_global("redeem_sweeper", &e.to_string()).await;
                }
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

/// Cookie refresh worker (Redis-coordinated, shared dev+prod).
///
/// Every `COOKIE_WORKER_INTERVAL_SECS` (default 10 min) it scans all profiles
/// sequentially: a profile with a live `cookie:fresh:{profile}` key in Redis is
/// skipped; otherwise the worker takes the `cookie:refreshing:{profile}` lock
/// (NX EX) and — only if it wins the lock — opens Firefox to refresh it. On a
/// successful refresh it writes a fresh key with TTL=`COOKIE_FRESH_TTL_SECS`
/// (default 36h) and releases the lock. On failure the lock is left to expire,
/// acting as a back-off so a broken profile is not retried every cycle.
///
/// If Redis is unreachable the cycle is skipped entirely (Mode A — never refresh
/// blindly) and the admin is notified once until Redis recovers.
pub fn spawn_cookie_refresher(api: &Bot, cookie_pool: &mut CookiePool) {
    let profiles: Vec<(String, String, String)> = cookie_pool
        .snapshot()
        .available_cookies
        .into_iter()
        .map(|c| {
            (
                c.profile_name,
                c.source_profile_dir.to_string_lossy().into_owned(),
                c.profile_dir.to_string_lossy().into_owned(),
            )
        })
        .collect();

    if profiles.is_empty() {
        println!("[cookie_worker] no profiles found, skipping.");
        return;
    }

    if !config::cookie_refresh_enabled() {
        println!("[cookie_worker] disabled via COOKIE_REFRESH_ENABLED=false, skipping.");
        return;
    }

    let admin_chat_id = config::admin_user_id().unwrap_or(0);
    let interval = config::cookie_worker_interval_secs();
    let fresh_ttl = config::cookie_fresh_ttl_secs();
    let lock_ttl = config::cookie_refresh_lock_ttl_secs();
    let redis_url = config::redis_url();
    let owner = config::env_label();
    let api = api.clone();

    println!(
        "[cookie_worker] starting: {} profile(s) interval={}s fresh_ttl={}s lock_ttl={}s owner={} redis={}",
        profiles.len(),
        interval,
        fresh_ttl,
        lock_ttl,
        owner,
        redis_url
    );

    tokio::spawn(async move {
        let store = match crate::cookie_pool::fresh::FreshStore::new(&redis_url) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[cookie_worker event=redis_client_init_failed] err={e}");
                crate::stats::record_error_global("cookie_worker", &e.to_string()).await;
                modules::notify_admin(&api, admin_chat_id, "⚠️ ساخت کلاینت Redis برای رفرش کوکی شکست خورد — رفرش کوکی‌ها غیرفعال شد. REDIS_URL رو چک کن.").await;
                return;
            }
        };

        // A freshly-started process can't be mid-refresh, so clear any refresh
        // locks left by a previous run of THIS env that died before unlocking
        // (prevents profiles being skip_locked for up to lock_ttl after a restart).
        match store.conn().await {
            Ok(mut conn) => match crate::cookie_pool::fresh::clear_own_locks(&mut conn, &owner)
                .await
            {
                Ok(n) if n > 0 => {
                    println!("[cookie_worker event=startup_lock_cleanup] owner={owner} removed={n}")
                }
                Ok(_) => {}
                Err(e) => eprintln!("[cookie_worker event=startup_lock_cleanup_failed] err={e}"),
            },
            Err(e) => eprintln!("[cookie_worker event=startup_lock_cleanup_failed] err={e}"),
        }

        let mut redis_down = false;
        loop {
            match cookie_worker_cycle(
                &api,
                &store,
                &profiles,
                &owner,
                fresh_ttl,
                lock_ttl,
                admin_chat_id,
            )
            .await
            {
                Ok(()) => {
                    if redis_down {
                        redis_down = false;
                        println!("[cookie_worker event=redis_recovered]");
                        modules::notify_admin(
                            &api,
                            admin_chat_id,
                            "✅ ارتباط با Redis دوباره برقرار شد — رفرش کوکی‌ها از سر گرفته شد.",
                        )
                        .await;
                    }
                }
                Err(e) => {
                    eprintln!("[cookie_worker event=redis_error] err={e}");
                    crate::stats::record_error_global("cookie_worker", &e.to_string()).await;
                    if !redis_down {
                        redis_down = true;
                        modules::notify_admin(&api, admin_chat_id, "⚠️ ارتباط با Redis قطع شده! رفرش کوکی‌ها متوقف شد. لطفاً Redis رو چک کن.").await;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(interval)).await;
        }
    });
}

/// One worker pass over all profiles. Returns Err only on a Redis failure, which
/// aborts the cycle (Mode A) so nothing is refreshed blindly.
async fn cookie_worker_cycle(
    api: &Bot,
    store: &crate::cookie_pool::fresh::FreshStore,
    profiles: &[(String, String, String)],
    owner: &str,
    fresh_ttl: u64,
    lock_ttl: u64,
    admin_chat_id: i64,
) -> redis::RedisResult<()> {
    use crate::cookie_pool::fresh;
    let mut conn = store.conn().await?; // Redis unreachable ⇒ Err ⇒ cycle aborts

    for (profile_name, profile_path, cache_dir) in profiles {
        if fresh::is_fresh(&mut conn, profile_name).await? {
            continue;
        }
        if !fresh::try_lock(&mut conn, profile_name, owner, lock_ttl).await? {
            println!(
                "[cookie_worker profile={profile_name} event=skip_locked] (another env refreshing)"
            );
            continue;
        }

        println!("[cookie_worker profile={profile_name} event=refresh_start] owner={owner}");
        let cfg = modules::cookie_refresher::CookieRefresherConfig {
            profile_path: profile_path.clone(),
            profile_name: profile_name.clone(),
            cache_dir: cache_dir.clone(),
            links_file: "files/youtube_links.txt".to_string(),
            duration_secs: 600,
            link_count: 1,
            admin_chat_id,
        };

        match modules::cookie_refresher::run(api, cfg).await {
            Ok(()) => {
                let ts = chrono::Utc::now().timestamp();
                if let Err(e) = fresh::mark_fresh(&mut conn, profile_name, fresh_ttl, ts).await {
                    eprintln!(
                        "[cookie_worker profile={profile_name} event=mark_fresh_failed] err={e}"
                    );
                    crate::stats::record_error_global("cookie_worker", &e.to_string()).await;
                }
                let _ = fresh::unlock(&mut conn, profile_name).await;
                println!(
                    "[cookie_worker profile={profile_name} event=refresh_done] fresh_ttl={fresh_ttl}s"
                );
            }
            Err(e) => {
                // Leave the lock to expire (back-off) instead of unlocking, so a
                // broken profile is not hammered every cycle.
                eprintln!(
                    "[cookie_worker profile={profile_name} event=refresh_failed] err={e} backoff={lock_ttl}s"
                );
                crate::stats::record_error_global("cookie_worker", &e.to_string()).await;
            }
        }
    }
    Ok(())
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
            eprintln!(
                "[cookie_refresh profile={p} event=cooldown_refresh_failed] cookie_id={cookie_id} err={e}"
            );
            crate::stats::record_error_global("cookie_refresh", &e.to_string()).await;
        } else {
            println!(
                "[cookie_refresh profile={p} event=cooldown_refresh_done] cookie_id={cookie_id}"
            );
        }
        let _ = done_tx.send(cookie_id);
    });
}

pub fn spawn_i18n_watcher() {
    // Not spawn_blocking: thread loop never terminates and Runtime::drop waits
    // for blocking tasks, hanging process on shutdown until SIGKILL (~90s).
    // Dedicated OS thread dies when main exits.
    std::thread::spawn(|| {
        use notify::{EventKind, RecursiveMode, Watcher, recommended_watcher};
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = recommended_watcher(tx).expect("failed to create file watcher");
        watcher
            .watch(
                std::path::Path::new("config/i18n.json"),
                RecursiveMode::NonRecursive,
            )
            .expect("failed to watch config/i18n.json");
        eprintln!("[i18n] watching config/i18n.json for changes");
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

    // 1. Set default commands first (no language_code) to display for all users
    let default_cmds = vec![
        BotCommand {
            command: "start".to_string(),
            description: t("commands.start"),
        },
        BotCommand {
            command: "panel".to_string(),
            description: t("commands.panel"),
        },
        BotCommand {
            command: "language".to_string(),
            description: t("commands.language"),
        },
        BotCommand {
            command: "rank".to_string(),
            description: t("commands.rank"),
        },
        BotCommand {
            command: "ref".to_string(),
            description: t("commands.ref"),
        },
    ];
    match api
        .set_my_commands(
            &SetMyCommandsParams::builder()
                .commands(default_cmds)
                .build(),
        )
        .await
    {
        Ok(_) => println!("Default bot commands set."),
        Err(e) => eprintln!("Failed to set default bot commands: {e}"),
    }

    // 2. Set specific commands per language
    for lang in ["fa", "en", "it", "ru"] {
        let commands = LANG
            .scope(lang.to_owned(), async {
                vec![
                    BotCommand {
                        command: "start".to_string(),
                        description: t("commands.start"),
                    },
                    BotCommand {
                        command: "panel".to_string(),
                        description: t("commands.panel"),
                    },
                    BotCommand {
                        command: "language".to_string(),
                        description: t("commands.language"),
                    },
                    BotCommand {
                        command: "rank".to_string(),
                        description: t("commands.rank"),
                    },
                    BotCommand {
                        command: "ref".to_string(),
                        description: t("commands.ref"),
                    },
                ]
            })
            .await;

        match api
            .set_my_commands(
                &SetMyCommandsParams::builder()
                    .commands(commands)
                    .language_code(lang.to_owned())
                    .build(),
            )
            .await
        {
            Ok(_) => println!("Bot commands set for lang={lang}."),
            Err(e) => eprintln!("Failed to set bot commands for lang={lang}: {e}"),
        }
    }
}
