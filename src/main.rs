use std::time::Duration;

mod bot;
mod config;
mod cookie_pool;
mod database;
mod emoji;
mod i18n;
mod youtube;

use bot::{send_text, send_start_button, START_BUTTON_CALLBACK};
use config::bot_token;
use cookie_pool::{CookiePool, format_cookie_status, format_selected_cookie, format_no_cookie_available, save_snapshot};
use database::postgresql::PostgresDatabase;
use emoji::{FlowManager, FlowState, handler as emoji_handler};
use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{AnswerCallbackQueryParams, GetUpdatesParams},
    types::MaybeInaccessibleMessage,
    updates::UpdateContent,
};
use i18n::t;
use youtube::{extract_youtube_urls, handle_youtube_url};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = bot_token()?;
    let api = Bot::new(&token);
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

    println!("Bot is running. Send /start to open the green button.");
    println!(
        "Cookie pool loaded: {} Firefox profile(s), {} selectable.",
        cookie_status.available_cookies, cookie_status.selectable_cookies
    );

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
                            "/start" => send_start_button(&api, message.chat.id).await?,
                            "/cookie_status" => {
                                let status = cookie_pool.status();
                                send_text(&api, message.chat.id, &format_cookie_status(&status)).await?;
                            }
                            "/cookie_next" => match cookie_pool.next_cookie() {
                                Some(cookie) => {
                                    save_snapshot(&database, &mut cookie_pool).await;
                                    send_text(&api, message.chat.id, &format_selected_cookie(&cookie)).await?;
                                }
                                None => {
                                    let status = cookie_pool.status();
                                    send_text(&api, message.chat.id, &format_no_cookie_available(&status)).await?;
                                }
                            },
                            "/cookie_429" => {
                                let text = match cookie_pool.mark_last_rate_limited() {
                                    Some(true) => { save_snapshot(&database, &mut cookie_pool).await; t("cookie.marked_429") }
                                    Some(false) => t("cookie.already_cooldown"),
                                    None => t("cookie.no_selection_yet"),
                                };
                                send_text(&api, message.chat.id, &text).await?;
                            }
                            _ => {
                                let urls = extract_youtube_urls(text);
                                for url in urls {
                                    handle_youtube_url(&api, message.chat.id, &url, &mut cookie_pool, &database).await;
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
                    if callback_query.data.as_deref() == Some(START_BUTTON_CALLBACK) {
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            api.send_message(
                                &frankenstein::methods::SendMessageParams::builder()
                                    .chat_id(message.chat.id)
                                    .text(t("start.hello"))
                                    .build(),
                            ).await?;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
