use std::sync::Arc;
use std::time::Duration;

mod bot;
mod config;
mod cookie_pool;
mod database;
mod emoji;
mod i18n;
mod youtube;

use bot::{send_text, send_start_menu, edit_to_start_menu, CB_START_EMOJI, CB_START_YOUTUBE};
use emoji::panel::CB_START_PANEL;
use config::bot_token;
use cookie_pool::CookiePool;
use database::postgresql::PostgresDatabase;
use emoji::{FlowManager, FlowState, handler as emoji_handler};
use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{AnswerCallbackQueryParams, GetUpdatesParams, SetChatMenuButtonParams, SetMyCommandsParams},
    methods::SendMessageParams,
    types::{BotCommand, ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, LinkPreviewOptions, MaybeInaccessibleMessage, MenuButton, ReplyMarkup},
    updates::UpdateContent,
};
use i18n::{t, reload_i18n};
use tokio::sync::RwLock;
use youtube::{extract_youtube_urls, handle_quality_callback, handle_youtube_url, log_trace, next_trace_id};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = bot_token()?;
    let api = build_bot_api(&token).await?;
    let mut cookie_pool = CookiePool::from_default_firefox();

    let database = match config::config_value("DATABASE_URL") {
        Some(database_url) => match PostgresDatabase::connect(&database_url).await {
            Ok(database) => {
                match database.load_state().await {
                    Ok((last_used_cookie, cooldown_list)) => {
                        cookie_pool.restore_state(last_used_cookie, cooldown_list);
                    }
                    Err(error) => eprintln!("failed to load cookie pool state: {error}"),
                }
                if let Err(error) = database.save_snapshot(&cookie_pool.snapshot()).await {
                    eprintln!("failed to save cookie pool snapshot: {error}");
                }
                println!("PostgreSQL cookie pool storage is enabled.");

                // Initialize emoji cache from admin DB
                if let Some(admin_id) = config::admin_user_id() {
                    let initial = emoji::cache::load_from_db(database.client(), admin_id).await;
                    let cache_arc = Arc::new(RwLock::new(initial));
                    let _ = emoji::cache::CACHE.set(cache_arc.clone());
                    println!("Emoji cache loaded for admin user {admin_id}.");

                    // Refresh every 5 minutes
                    tokio::spawn(async move {
                        let refresh_client = match tokio_postgres::connect(&database_url, tokio_postgres::NoTls).await {
                            Ok((c, conn)) => {
                                tokio::spawn(conn);
                                c
                            }
                            Err(e) => {
                                eprintln!("emoji cache refresh: failed to connect: {e}");
                                return;
                            }
                        };
                        loop {
                            tokio::time::sleep(Duration::from_secs(300)).await;
                            let fresh = emoji::cache::load_from_db(&refresh_client, admin_id).await;
                            *cache_arc.write().await = fresh;
                            println!("Emoji cache refreshed.");
                        }
                    });
                } else {
                    println!("ADMIN_USER_ID not set; emoji cache disabled.");
                }

                Some(database)
            }
            Err(error) => {
                eprintln!("failed to connect to PostgreSQL: {error}");
                None
            }
        },
        None => {
            println!("DATABASE_URL is not set; cookie pool state is in-memory only.");
            None
        }
    };

    let cookie_status = cookie_pool.status();
    let mut flow_manager = FlowManager::new();
    let mut params = GetUpdatesParams::builder().timeout(30u32).build();

    // Watch i18n.json and auto-reload on change
    tokio::task::spawn_blocking(|| {
        use notify::{Watcher, RecursiveMode, recommended_watcher, Event, EventKind};
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = recommended_watcher(tx).expect("failed to create file watcher");
        watcher.watch(std::path::Path::new("i18n.json"), RecursiveMode::NonRecursive)
            .expect("failed to watch i18n.json");
        eprintln!("[i18n] watching i18n.json for changes");
        for res in rx {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Modify(_) | EventKind::Create(_) => {
                        reload_i18n();
                    }
                    _ => {}
                }
            }
        }
    });

    println!("Bot is running. Send /start to open the green button.");
    println!(
        "Cookie pool loaded: {} Firefox profile(s), {} selectable.",
        cookie_status.available_cookies, cookie_status.selectable_cookies
    );

    let menu_params = SetChatMenuButtonParams::builder()
        .menu_button(MenuButton::Commands)
        .build();
    match api.set_chat_menu_button(&menu_params).await {
        Ok(_) => println!("Chat menu button set to Commands."),
        Err(e) => eprintln!("Failed to set chat menu button: {e}"),
    }

    let commands = vec![
        BotCommand {
            command: "start".to_string(),
            description: "منوی اصلی".to_string(),
        },
        BotCommand {
            command: "emoji".to_string(),
            description: "پنل مدیریت ایموجی".to_string(),
        },
        BotCommand {
            command: "se".to_string(),
            description: "تنظیم لقب ایموجی".to_string(),
        },
    ];
    match api.set_my_commands(&SetMyCommandsParams::builder().commands(commands).build()).await {
        Ok(_) => println!("Bot commands set successfully."),
        Err(e) => eprintln!("Failed to set bot commands: {e}"),
    }

    loop {
        let updates = match api.get_updates(&params).await {
            Ok(response) => response.result,
            Err(error) => {
                eprintln!("get_updates failed: {error}");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        for update in updates {
            params.offset = Some((update.update_id + 1) as i64);

            match update.content {
                UpdateContent::Message(message) => {
                    let user_id = message.from.as_ref().map(|u| u.id as i64);

                    if let Some(uid) = user_id {
                        if let Some(text) = message.text.as_deref() {
                            if !text.trim_start().starts_with('/') {
                                if let Some(pack_name) = emoji_handler::extract_addemoji_pack_name(text) {
                                    emoji_handler::handle_addemoji_link(
                                        &api, &message, uid, &pack_name, &mut flow_manager, &database,
                                    ).await;
                                    continue;
                                }
                            }
                        }
                    }

                    if user_id.is_some()
                        && !matches!(flow_manager.get(user_id.unwrap()), FlowState::Idle)
                    {
                        if emoji_handler::handle_emoji_flow_message(
                            &api, &message, user_id.unwrap(), &mut flow_manager, &database,
                        ).await {
                            continue;
                        }
                    }

                    if let Some(text) = message.text.as_deref() {
                        if text == "/emoji" {
                            emoji_handler::handle_emoji_command(&api, &message, &mut flow_manager, &database).await;
                            continue;
                        }
                        if let Some(rest) = text.strip_prefix("/se") {
                            emoji_handler::handle_se_command(&api, &message, rest, &database).await;
                            continue;
                        }
                        match text {
                            "/i18n_reload" => {
                                let is_admin = config::admin_user_id()
                                    .map(|id| Some(id) == user_id)
                                    .unwrap_or(false);
                                if is_admin {
                                    reload_i18n();
                                    send_text(&api, message.chat.id, "✅ i18n.json reloaded.").await?;
                                }
                            }
                            "/start" => send_start_menu(&api, message.chat.id).await?,
                            _ => {
                                let urls = extract_youtube_urls(text);
                                for url in urls {
                                    let trace_id = next_trace_id();
                                    log_trace(trace_id, "route_youtube_url", &format!("user_id={user_id:?} chat_id={} url={url}", message.chat.id));
                                    handle_youtube_url(&api, message.chat.id, message.message_id, user_id, trace_id, &url, &mut cookie_pool, &database).await;
                                }
                            }
                        }
                    }
                }
                UpdateContent::CallbackQuery(callback_query) => {
                    if callback_query.data.as_deref().map(|d| d.starts_with("emoji:")).unwrap_or(false) {
                        emoji_handler::handle_emoji_callback(&api, &callback_query, &mut flow_manager, &database).await;
                        continue;
                    }
                    if handle_quality_callback(&api, &callback_query).await {
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_START_EMOJI) {
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            emoji_handler::open_emoji_panel(
                                &api, message.chat.id, callback_query.from.id as i64,
                                &mut flow_manager, &database,
                            ).await;
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_START_YOUTUBE) {
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            let icon_id = t("emoji.panel.icons.back");
                            let back_btn = InlineKeyboardButton {
                                text: t("start.back"),
                                icon_custom_emoji_id: if icon_id.is_empty() || icon_id.starts_with('!') { None } else { Some(icon_id) },
                                callback_data: Some(CB_START_PANEL.to_string()),
                                style: Some(ButtonStyle::Primary),
                                url: None, login_url: None, web_app: None,
                                switch_inline_query: None, switch_inline_query_current_chat: None,
                                switch_inline_query_chosen_chat: None, copy_text: None,
                                callback_game: None, pay: None,
                            };
                            let keyboard = InlineKeyboardMarkup::builder()
                                .inline_keyboard(vec![vec![back_btn]])
                                .build();
                            let no_preview = LinkPreviewOptions::builder().is_disabled(true).build();
                            let params = SendMessageParams::builder()
                                .chat_id(message.chat.id)
                                .text(t("start.youtube_info"))
                                .link_preview_options(no_preview)
                                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard))
                                .build();
                            let _ = api.send_message(&params).await;
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_START_PANEL) {
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            let _ = edit_to_start_menu(&api, message.chat.id, message.message_id).await;
                        }
                        continue;
                    }
                }
                _ => {}
            }
        }
    }
}

async fn build_bot_api(token: &str) -> Result<Bot, Box<dyn std::error::Error>> {
    let Some(base_url) = config::bot_api_base_url() else {
        println!("BOT_API_BASE_URL is not set; using official Telegram Bot API.");
        return Ok(Bot::new(token));
    };

    let base_url = base_url.trim_end_matches('/').to_string();
    if is_local_bot_api_url(&base_url) {
        println!("Local Bot API base detected ({base_url}); logging out from official Telegram Bot API.");
        let official_api = Bot::new(token);
        match official_api.log_out().await {
            Ok(response) => {
                println!("Official Telegram Bot API logOut result: {}", response.result);
            }
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

fn is_local_bot_api_url(base_url: &str) -> bool {
    base_url.contains("127.0.0.1") || base_url.contains("localhost")
}
