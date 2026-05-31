use std::{env, fs, time::Duration};

mod cookie_pool;
mod database;

use cookie_pool::{CookiePool, CookiePoolStatus, SelectedCookie};
use database::postgresql::PostgresDatabase;
use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{AnswerCallbackQueryParams, GetUpdatesParams, SendMessageParams},
    types::{
        ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, MaybeInaccessibleMessage,
        ReplyMarkup,
    },
    updates::UpdateContent,
};

const START_BUTTON_CALLBACK: &str = "say_hello";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = bot_token()?;
    let api = Bot::new(&token);
    let mut cookie_pool = CookiePool::from_default_firefox();

    let database = match config_value("DATABASE_URL") {
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
                    if let Some(text) = message.text.as_deref() {
                        match text {
                            "/start" => send_start_button(&api, message.chat.id).await?,
                            "/cookie_status" => {
                                let status = cookie_pool.status();
                                send_text(&api, message.chat.id, &format_cookie_status(&status))
                                    .await?;
                            }
                            "/cookie_next" => match cookie_pool.next_cookie() {
                                Some(cookie) => {
                                    save_cookie_pool_snapshot(&database, &mut cookie_pool).await;
                                    send_text(
                                        &api,
                                        message.chat.id,
                                        &format_selected_cookie(&cookie),
                                    )
                                    .await?;
                                }
                                None => {
                                    let status = cookie_pool.status();
                                    send_text(
                                        &api,
                                        message.chat.id,
                                        &format_no_cookie_available(&status),
                                    )
                                    .await?;
                                }
                            },
                            "/cookie_429" => {
                                let text = match cookie_pool.mark_last_rate_limited() {
                                    Some(true) => {
                                        save_cookie_pool_snapshot(&database, &mut cookie_pool)
                                            .await;
                                        "آخرین کوکی انتخاب‌شده وارد cooldown بیست ساعته شد."
                                    }
                                    Some(false) => {
                                        "آخرین کوکی از قبل داخل cooldown است یا cooldown پر شده."
                                    }
                                    None => "هنوز هیچ کوکی‌ای انتخاب نشده است.",
                                };

                                send_text(&api, message.chat.id, text).await?;
                            }
                            _ => {}
                        }
                    }
                }
                UpdateContent::CallbackQuery(callback_query) => {
                    if callback_query.data.as_deref() == Some(START_BUTTON_CALLBACK) {
                        let answer_params = AnswerCallbackQueryParams::builder()
                            .callback_query_id(callback_query.id)
                            .build();
                        api.answer_callback_query(&answer_params).await?;

                        if let Some(MaybeInaccessibleMessage::Message(message)) =
                            callback_query.message
                        {
                            api.send_message(
                                &SendMessageParams::builder()
                                    .chat_id(message.chat.id)
                                    .text("سلام")
                                    .build(),
                            )
                            .await?;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn format_cookie_status(status: &CookiePoolStatus) -> String {
    let last_used = status.last_used_cookie.as_deref().unwrap_or("-");
    let wait = status
        .next_available_in
        .map(format_duration)
        .unwrap_or_else(|| "-".to_owned());

    format!(
        "Cookie Pool\navailable_cookies: {}\nselectable_cookies: {}\ncooldown_list: {}\nlast_used_cookie: {}\nnext_available_in: {}",
        status.available_cookies,
        status.selectable_cookies,
        status.cooldown_cookies,
        last_used,
        wait
    )
}

fn format_selected_cookie(cookie: &SelectedCookie) -> String {
    format!(
        "selected_cookie: {}\nprofile: {}\ncookies_file: {}\nyt-dlp:\nyt-dlp --cookies-from-browser '{}'",
        cookie.id,
        cookie.profile_name,
        cookie.cookies_file.display(),
        cookie.yt_dlp_browser_spec
    )
}

fn format_no_cookie_available(status: &CookiePoolStatus) -> String {
    let wait = status
        .next_available_in
        .map(format_duration)
        .unwrap_or_else(|| "20h".to_owned());

    format!("کوکی قابل استفاده موجود نیست. باید حدود {wait} صبر کرد.")
}

fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m {seconds}s")
    }
}

fn bot_token() -> Result<String, Box<dyn std::error::Error>> {
    config_value("BOT_TOKEN")
        .ok_or_else(|| "BOT_TOKEN is not set in .env, /etc/default/abc, or process env".into())
}

fn config_value(key: &str) -> Option<String> {
    value_from_env_file(".env", key)
        .or_else(|| value_from_env_file("/etc/default/abc", key))
        .or_else(|| env::var(key).ok())
}

fn value_from_env_file(path: &str, target_key: &str) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;

    contents.lines().find_map(|line| {
        let line = line.trim();

        if line.is_empty() || line.starts_with('#') {
            return None;
        }

        let (key, value) = line.split_once('=')?;

        if key.trim() != target_key {
            return None;
        }

        let token = unquote_env_value(value.trim());

        if token.is_empty() {
            None
        } else {
            Some(token.to_owned())
        }
    })
}

async fn save_cookie_pool_snapshot(
    database: &Option<PostgresDatabase>,
    cookie_pool: &mut CookiePool,
) {
    let Some(database) = database else {
        return;
    };

    if let Err(error) = database.save_snapshot(&cookie_pool.snapshot()).await {
        eprintln!("failed to save cookie pool snapshot: {error}");
    }
}

fn unquote_env_value(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[value.len() - 1];

        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &value[1..value.len() - 1];
        }
    }

    value
}

async fn send_start_button(
    api: &Bot,
    chat_id: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let button = InlineKeyboardButton::builder()
        .text("نمایش سلام")
        .callback_data(START_BUTTON_CALLBACK)
        .style(ButtonStyle::Success)
        .build();

    let keyboard = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![button]])
        .build();

    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text("روی دکمه بزن:")
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard))
        .build();

    api.send_message(&params).await?;
    Ok(())
}

async fn send_text(
    api: &Bot,
    chat_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .build(),
    )
    .await?;

    Ok(())
}
