use std::{collections::HashSet, env, fs, time::Duration};

mod cookie_pool;
mod database;
mod emoji;
mod i18n;
mod youtube;

use i18n::{t, tf};

use cookie_pool::{CookiePool, CookiePoolStatus, SelectedCookie};
use database::postgresql::PostgresDatabase;
use emoji::{
    FlowManager, FlowState, PendingEmoji,
    panel::{
        self as emoji_panel, CB_ADD, CB_BACK, CB_CANCEL, CB_DELETE_PACK_MENU, CB_EXPORT, CB_IMPORT,
        CB_LIST, CB_PACKS, CB_PACK_DELETE_PREFIX, CB_PACK_OPEN_PREFIX,
        CB_PACK_SET_ALIAS_PREFIX, CB_PACK_SET_DEFAULT_PREFIX, CB_TEST,
    },
    store as emoji_store,
};
use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::FileUpload,
    methods::{
        AnswerCallbackQueryParams, EditMessageTextParams, GetUpdatesParams, SendMessageParams,
        SendPhotoParams,
    },
    types::{
        ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, LinkPreviewOptions,
        MaybeInaccessibleMessage, Message, MessageEntity, MessageEntityType, ReplyMarkup,
        ReplyKeyboardRemove,
    },
    updates::UpdateContent,
};
use youtube::{
    FetchError, build_caption, build_description_blockquotes, extract_youtube_urls,
    fetch_video_info,
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
                        if handle_emoji_flow_message(
                            &api,
                            &message,
                            user_id.unwrap(),
                            &mut flow_manager,
                            &database,
                        )
                        .await
                        {
                            continue;
                        }
                    }

                    if let Some(text) = message.text.as_deref() {
                        if text == "/emoji" {
                            handle_emoji_command(&api, &message, &database).await;
                            continue;
                        }
                        if let Some(rest) = text.strip_prefix("/se") {
                            handle_se_command(&api, &message, rest, &database).await;
                            continue;
                        }
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
                                        t("cookie.marked_429")
                                    }
                                    Some(false) => t("cookie.already_cooldown"),
                                    None => t("cookie.no_selection_yet"),
                                };

                                send_text(&api, message.chat.id, &text).await?;
                            }
                            _ => {
                                let urls = extract_youtube_urls(text);
                                for url in urls {
                                    handle_youtube_url(
                                        &api,
                                        message.chat.id,
                                        &url,
                                        &mut cookie_pool,
                                        &database,
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                }
                UpdateContent::CallbackQuery(callback_query) => {
                    if callback_query
                        .data
                        .as_deref()
                        .map(|d| d.starts_with("emoji:"))
                        .unwrap_or(false)
                    {
                        handle_emoji_callback(
                            &api,
                            &callback_query,
                            &mut flow_manager,
                            &database,
                        )
                        .await;
                        continue;
                    }
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
                                    .text(t("start.hello"))
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

    let available = status.available_cookies.to_string();
    let selectable = status.selectable_cookies.to_string();
    let cooldown = status.cooldown_cookies.to_string();

    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        t("cookie.status_header"),
        tf("cookie.status_line_available", &[("available", &available)]),
        tf("cookie.status_line_selectable", &[("selectable", &selectable)]),
        tf("cookie.status_line_cooldown", &[("cooldown", &cooldown)]),
        tf("cookie.status_line_last_used", &[("last_used", last_used)]),
        tf("cookie.status_line_next_available", &[("wait", &wait)]),
    )
}

fn format_selected_cookie(cookie: &SelectedCookie) -> String {
    tf(
        "cookie.selected",
        &[
            ("id", &cookie.id),
            ("profile", &cookie.profile_name),
            ("file", &cookie.cookies_file.display().to_string()),
            ("spec", &cookie.yt_dlp_browser_spec),
        ],
    )
}

fn format_no_cookie_available(status: &CookiePoolStatus) -> String {
    let wait = status
        .next_available_in
        .map(format_duration)
        .unwrap_or_else(|| "20h".to_owned());

    tf("cookie.none_available", &[("wait", &wait)])
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
        .text(t("start.button"))
        .callback_data(START_BUTTON_CALLBACK)
        .style(ButtonStyle::Success)
        .build();

    let keyboard = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![button]])
        .build();

    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(t("start.prompt"))
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

async fn send_cancel_and_panel(api: &Bot, chat_id: i64) {
    let _ = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(t("emoji.canceled"))
                .reply_markup(ReplyMarkup::ReplyKeyboardRemove(
                    ReplyKeyboardRemove::builder().remove_keyboard(true).build(),
                ))
                .build(),
        )
        .await;
    let _ = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(emoji_panel::main_panel_text())
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(
                    emoji_panel::main_panel_keyboard(),
                ))
                .build(),
        )
        .await;
}

async fn send_text_md(
    api: &Bot,
    chat_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .parse_mode(ParseMode::MarkdownV2)
            .build(),
    )
    .await?;

    Ok(())
}

async fn handle_youtube_url(
    api: &Bot,
    chat_id: i64,
    url: &str,
    cookie_pool: &mut CookiePool,
    database: &Option<PostgresDatabase>,
) {
    let mut tried: HashSet<String> = HashSet::new();
    loop {
        let cookie = match cookie_pool.next_cookie() {
            Some(c) => c,
            None => {
                let status = cookie_pool.status();
                let _ = send_text(api, chat_id, &format_no_cookie_available(&status)).await;
                return;
            }
        };
        if tried.contains(&cookie.id) {
            let status = cookie_pool.status();
            let _ = send_text(api, chat_id, &format_no_cookie_available(&status)).await;
            return;
        }
        tried.insert(cookie.id.clone());
        save_cookie_pool_snapshot(database, cookie_pool).await;

        match fetch_video_info(url, &cookie.yt_dlp_browser_spec).await {
            Ok(info) => {
                let caption = build_caption(&info);
                let photo = info
                    .thumbnail
                    .clone()
                    .unwrap_or_else(|| info.webpage_url.clone());

                let params = SendPhotoParams::builder()
                    .chat_id(chat_id)
                    .photo(FileUpload::String(photo))
                    .caption(caption)
                    .parse_mode(ParseMode::MarkdownV2)
                    .build();

                if let Err(error) = api.send_photo(&params).await {
                    eprintln!("send_photo failed: {error}");
                    let _ = send_text(
                        api,
                        chat_id,
                        &tf("youtube.send_photo_failed", &[("error", &error.to_string())]),
                    )
                    .await;
                    return;
                }

                if let Some(desc) = info.description.as_deref() {
                    if desc.chars().count() > 1000 {
                        let link_preview = LinkPreviewOptions::builder()
                            .is_disabled(true)
                            .build();
                        for chunk in build_description_blockquotes(desc) {
                            let msg = SendMessageParams::builder()
                                .chat_id(chat_id)
                                .text(chunk)
                                .parse_mode(ParseMode::MarkdownV2)
                                .link_preview_options(link_preview.clone())
                                .build();
                            if let Err(error) = api.send_message(&msg).await {
                                eprintln!("send description chunk failed: {error}");
                                break;
                            }
                        }
                    }
                }
                return;
            }
            Err(FetchError::RateLimited) => {
                if cookie_pool.mark_last_rate_limited() == Some(true) {
                    save_cookie_pool_snapshot(database, cookie_pool).await;
                }
                eprintln!("yt-dlp 429 with cookie {}; retrying", cookie.id);
                continue;
            }
            Err(FetchError::BadCookie(msg)) => {
                eprintln!("bad cookie {}: {msg}; trying next", cookie.id);
                continue;
            }
            Err(FetchError::Other(msg)) => {
                eprintln!("yt-dlp failed for {url}: {msg}");
                let _ = send_text(
                    api,
                    chat_id,
                    &tf("youtube.fetch_failed", &[("error", &msg)]),
                )
                .await;
                return;
            }
        }
    }
}

async fn handle_emoji_command(
    api: &Bot,
    message: &Message,
    database: &Option<PostgresDatabase>,
) {
    if database.is_none() {
        let _ = send_text(api, message.chat.id, &t("emoji.db_required")).await;
        return;
    }
    let params = SendMessageParams::builder()
        .chat_id(message.chat.id)
        .text(emoji_panel::main_panel_text())
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(
            emoji_panel::main_panel_keyboard(),
        ))
        .build();
    if let Err(error) = api.send_message(&params).await {
        eprintln!("send emoji panel failed: {error}");
    }
}

async fn handle_emoji_callback(
    api: &Bot,
    cbq: &frankenstein::types::CallbackQuery,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let _ = api
        .answer_callback_query(
            &AnswerCallbackQueryParams::builder()
                .callback_query_id(&cbq.id)
                .build(),
        )
        .await;

    let Some(data) = cbq.data.as_deref() else {
        return;
    };
    let Some(MaybeInaccessibleMessage::Message(panel_msg)) = cbq.message.clone() else {
        return;
    };
    let chat_id = panel_msg.chat.id;
    let message_id = panel_msg.message_id;
    let user_id = cbq.from.id as i64;
    let Some(db) = database else {
        let _ = send_text(api, chat_id, &t("emoji.db_required")).await;
        return;
    };
    let client = db.client();

    match data {
        d if d == CB_ADD => {
            flow_manager.set(
                user_id,
                FlowState::AwaitingEmojis {
                    collected: Vec::new(),
                },
            );
            let params = SendMessageParams::builder()
                .chat_id(chat_id)
                .text(t("emoji.add_prompt"))
                .reply_markup(ReplyMarkup::ReplyKeyboardMarkup(
                    emoji_panel::cancel_reply_keyboard(),
                ))
                .build();
            let _ = api.send_message(&params).await;
        }
        d if d == CB_TEST => {
            flow_manager.set(user_id, FlowState::AwaitingTestText);
            let _ = send_text(api, chat_id, &t("emoji.test_prompt")).await;
        }
        d if d == CB_LIST => {
            send_emoji_list(api, chat_id, user_id, client).await;
        }
        d if d == CB_PACKS || d == CB_DELETE_PACK_MENU => {
            show_packs_menu(api, chat_id, message_id, user_id, client).await;
        }
        d if d == CB_IMPORT || d == CB_EXPORT => {
            let _ = send_text(api, chat_id, "🚧 به‌زودی").await;
        }
        d if d == CB_BACK || d == CB_CANCEL => {
            flow_manager.clear(user_id);
            edit_panel(
                api,
                chat_id,
                message_id,
                &emoji_panel::main_panel_text(),
                Some(emoji_panel::main_panel_keyboard()),
            )
            .await;
        }
        d if d.starts_with(emoji_panel::CB_LIST_PAGE_PREFIX) => {
            if let Some(page) = d
                .strip_prefix(emoji_panel::CB_LIST_PAGE_PREFIX)
                .and_then(|s| s.parse::<usize>().ok())
            {
                edit_emoji_list_page(api, chat_id, message_id, user_id, client, page).await;
            }
        }
        d if d.starts_with(CB_PACK_OPEN_PREFIX) => {
            if let Some(pack_id) = d
                .strip_prefix(CB_PACK_OPEN_PREFIX)
                .and_then(|s| s.parse::<i32>().ok())
            {
                show_pack_detail(api, chat_id, message_id, user_id, pack_id, client).await;
            }
        }
        d if d.starts_with(CB_PACK_SET_DEFAULT_PREFIX) => {
            if let Some(pack_id) = d
                .strip_prefix(CB_PACK_SET_DEFAULT_PREFIX)
                .and_then(|s| s.parse::<i32>().ok())
            {
                if let Err(error) = emoji_store::set_default_pack(client, user_id, pack_id).await {
                    eprintln!("set_default_pack failed: {error}");
                }
                show_pack_detail(api, chat_id, message_id, user_id, pack_id, client).await;
            }
        }
        d if d.starts_with(CB_PACK_SET_ALIAS_PREFIX) => {
            if let Some(pack_id) = d
                .strip_prefix(CB_PACK_SET_ALIAS_PREFIX)
                .and_then(|s| s.parse::<i32>().ok())
            {
                flow_manager.set(user_id, FlowState::AwaitingPackAlias { pack_id });
                let _ = send_text(api, chat_id, &t("emoji.pack_alias_prompt")).await;
            }
        }
        d if d.starts_with(CB_PACK_DELETE_PREFIX) => {
            if let Some(pack_id) = d
                .strip_prefix(CB_PACK_DELETE_PREFIX)
                .and_then(|s| s.parse::<i32>().ok())
            {
                let name = emoji_store::list_packs(client, user_id)
                    .await
                    .ok()
                    .and_then(|packs| packs.into_iter().find(|p| p.id == pack_id))
                    .map(|p| p.name)
                    .unwrap_or_default();
                if let Err(error) = emoji_store::delete_pack(client, user_id, pack_id).await {
                    eprintln!("delete_pack failed: {error}");
                }
                let _ = send_text(
                    api,
                    chat_id,
                    &tf("emoji.pack_deleted", &[("name", &name)]),
                )
                .await;
                show_packs_menu(api, chat_id, message_id, user_id, client).await;
            }
        }
        _ => {}
    }
}

async fn handle_emoji_flow_message(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) -> bool {
    let chat_id = message.chat.id;
    let Some(db) = database else {
        return false;
    };
    let client = db.client();
    let state = flow_manager.get(user_id);

    match state {
        FlowState::Idle => false,
        FlowState::AwaitingEmojis { mut collected } => {
            let msg_text = message.text.as_deref().unwrap_or("").trim();
            if msg_text == t("emoji.cancel_button") {
                flow_manager.clear(user_id);
                send_cancel_and_panel(api, chat_id).await;
                return true;
            }
            let mut new_emojis = extract_custom_emojis(message);
            if new_emojis.is_empty() && collected.is_empty() {
                let _ = send_text(api, chat_id, &t("emoji.no_emoji_found")).await;
                return true;
            }
            let incoming_count = new_emojis.len();
            let duplicates = filter_duplicates(client, user_id, &mut new_emojis, &collected).await;
            if incoming_count > 0 && new_emojis.is_empty() && collected.is_empty() {
                let _ = send_all_duplicate_message(api, chat_id, &duplicates).await;
                flow_manager.set(user_id, FlowState::AwaitingEmojis { collected });
                return true;
            }
            collected.append(&mut new_emojis);
            let text = emoji_panel::format_pending_emojis(&collected, &duplicates);
            let _ = send_text_md(api, chat_id, &text).await;
            flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
            true
        }
        FlowState::AwaitingPackChoice { mut collected } => {
            let text = message.text.as_deref().unwrap_or("").trim();

            if text == t("emoji.cancel_button") {
                flow_manager.clear(user_id);
                send_cancel_and_panel(api, chat_id).await;
                return true;
            }

            let mut extras = extract_custom_emojis(message);
            if !extras.is_empty() {
                let incoming = extras.len();
                let duplicates =
                    filter_duplicates(client, user_id, &mut extras, &collected).await;
                if incoming > 0 && extras.is_empty() {
                    let _ = send_all_duplicate_message(api, chat_id, &duplicates).await;
                    flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
                    return true;
                }
                collected.extend(extras);
                let summary = emoji_panel::format_pending_emojis(&collected, &duplicates);
                let _ = send_text_md(api, chat_id, &summary).await;
                flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
                return true;
            }

            if text.starts_with('-') || text.starts_with('+') {
                if let Err(_) = apply_edit_ops(&mut collected, text) {
                    let _ = send_text(api, chat_id, &t("emoji.pending.mixed_ops")).await;
                    flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
                    return true;
                }
                let summary = emoji_panel::format_pending_emojis(&collected, &[]);
                let _ = send_text_md(api, chat_id, &summary).await;
                flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
                return true;
            }

            if text.is_empty() {
                return true;
            }

            let pack = match emoji_store::find_pack_by_name(client, user_id, text).await {
                Ok(Some(p)) => p,
                Ok(None) => match emoji_store::create_pack(client, user_id, text).await {
                    Ok(p) => p,
                    Err(error) => {
                        eprintln!("create_pack failed: {error}");
                        flow_manager.clear(user_id);
                        return true;
                    }
                },
                Err(error) => {
                    eprintln!("find_pack_by_name failed: {error}");
                    flow_manager.clear(user_id);
                    return true;
                }
            };

            let mut added = 0;
            for emoji in &collected {
                let smart = match emoji_store::allocate_smart_name(
                    client,
                    user_id,
                    &emoji.fallback,
                )
                .await
                {
                    Ok(s) => s,
                    Err(error) => {
                        eprintln!("allocate_smart_name failed: {error}");
                        continue;
                    }
                };
                if let Err(error) = emoji_store::add_item(
                    client,
                    user_id,
                    pack.id,
                    &emoji.custom_emoji_id,
                    &emoji.fallback,
                    &smart,
                )
                .await
                {
                    eprintln!("add_item failed: {error}");
                    continue;
                }
                added += 1;
            }

            let _ = send_text(
                api,
                chat_id,
                &tf(
                    "emoji.added_summary",
                    &[("count", &added.to_string()), ("pack", &pack.name)],
                ),
            )
            .await;
            flow_manager.clear(user_id);
            true
        }
        FlowState::AwaitingPackAlias { pack_id } => {
            let text = message.text.as_deref().unwrap_or("").trim();
            let alias = if text == "-" || text.is_empty() {
                None
            } else {
                Some(text)
            };
            if let Err(error) = emoji_store::set_pack_alias(client, user_id, pack_id, alias).await
            {
                eprintln!("set_pack_alias failed: {error}");
            }
            let _ = send_text(api, chat_id, &t("emoji.pack_alias_set")).await;
            flow_manager.clear(user_id);
            true
        }
        FlowState::AwaitingTestText => {
            let raw = message.text.as_deref().unwrap_or("");
            let rendered = format!("(تست template هنوز پیاده نشده)\n\n{raw}");
            let _ = send_text(api, chat_id, &rendered).await;
            flow_manager.clear(user_id);
            true
        }
    }
}

async fn handle_se_command(
    api: &Bot,
    message: &Message,
    rest: &str,
    database: &Option<PostgresDatabase>,
) {
    let chat_id = message.chat.id;
    let Some(user) = message.from.as_ref() else {
        return;
    };
    let user_id = user.id as i64;

    let mut parts = rest.split_whitespace();
    let selector = parts.next();
    let alias = parts.next();
    let (Some(selector), Some(alias)) = (selector, alias) else {
        let _ = send_text(api, chat_id, &t("emoji.se_usage")).await;
        return;
    };

    let Some(db) = database else {
        let _ = send_text(api, chat_id, &t("emoji.db_required")).await;
        return;
    };
    let client = db.client();

    let alias_value = if alias == "-" { None } else { Some(alias) };
    match emoji_store::set_item_alias(client, user_id, selector, alias_value).await {
        Ok(true) => {
            let _ = send_text(
                api,
                chat_id,
                &tf("emoji.se_done", &[("alias", alias)]),
            )
            .await;
        }
        Ok(false) => {
            let _ = send_text(api, chat_id, &t("emoji.se_not_found")).await;
        }
        Err(error) => {
            eprintln!("set_item_alias failed: {error}");
        }
    }
}

async fn send_emoji_list(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    client: &tokio_postgres::Client,
) {
    let packs = match emoji_store::list_packs(client, user_id).await {
        Ok(p) => p,
        Err(error) => {
            eprintln!("list_packs failed: {error}");
            return;
        }
    };
    if packs.is_empty() {
        let _ = send_text(api, chat_id, &t("emoji.no_packs")).await;
        return;
    }
    let mut packs_with_items: Vec<(emoji_store::EmojiPack, Vec<emoji_store::EmojiItem>)> =
        Vec::new();
    for pack in packs {
        let items = emoji_store::list_items(client, pack.id).await.unwrap_or_default();
        packs_with_items.push((pack, items));
    }
    let (text, page, total_pages) = emoji_panel::build_list_page(&packs_with_items, 0);
    let keyboard = emoji_panel::list_page_keyboard(page, total_pages);
    let _ = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(keyboard))
                .build(),
        )
        .await;
}

async fn edit_emoji_list_page(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    client: &tokio_postgres::Client,
    page: usize,
) {
    let packs = match emoji_store::list_packs(client, user_id).await {
        Ok(p) => p,
        Err(error) => {
            eprintln!("list_packs failed: {error}");
            return;
        }
    };
    let mut packs_with_items: Vec<(emoji_store::EmojiPack, Vec<emoji_store::EmojiItem>)> =
        Vec::new();
    for pack in packs {
        let items = emoji_store::list_items(client, pack.id).await.unwrap_or_default();
        packs_with_items.push((pack, items));
    }
    let (text, page, total_pages) = emoji_panel::build_list_page(&packs_with_items, page);
    let keyboard = emoji_panel::list_page_keyboard(page, total_pages);
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .build();
    if let Err(error) = api.edit_message_text(&params).await {
        eprintln!("edit_message_text failed: {error}");
    }
}

async fn show_packs_menu(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    client: &tokio_postgres::Client,
) {
    let packs = match emoji_store::list_packs(client, user_id).await {
        Ok(p) => p,
        Err(error) => {
            eprintln!("list_packs failed: {error}");
            return;
        }
    };
    if packs.is_empty() {
        let _ = send_text(api, chat_id, &t("emoji.no_packs")).await;
        return;
    }
    edit_panel(
        api,
        chat_id,
        message_id,
        "📁 مجموعه‌ها:",
        Some(emoji_panel::packs_keyboard(&packs)),
    )
    .await;
}

async fn show_pack_detail(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    pack_id: i32,
    client: &tokio_postgres::Client,
) {
    let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
    let Some(pack) = packs.into_iter().find(|p| p.id == pack_id) else {
        return;
    };
    edit_panel(
        api,
        chat_id,
        message_id,
        &emoji_panel::pack_detail_text(&pack),
        Some(emoji_panel::pack_detail_keyboard(&pack)),
    )
    .await;
}

async fn edit_panel(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    text: &str,
    keyboard: Option<InlineKeyboardMarkup>,
) {
    let builder = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text);
    let params = match keyboard {
        Some(kb) => builder.reply_markup(kb).build(),
        None => builder.build(),
    };
    if let Err(error) = api.edit_message_text(&params).await {
        eprintln!("edit_message_text failed: {error}");
    }
}

fn extract_custom_emojis(message: &Message) -> Vec<PendingEmoji> {
    let mut out = Vec::new();
    let text = message.text.as_deref().unwrap_or("");
    if let Some(entities) = &message.entities {
        for entity in entities {
            push_custom_emoji(&mut out, text, entity);
        }
    }
    let caption = message.caption.as_deref().unwrap_or("");
    if let Some(entities) = &message.caption_entities {
        for entity in entities {
            push_custom_emoji(&mut out, caption, entity);
        }
    }
    out
}

fn push_custom_emoji(out: &mut Vec<PendingEmoji>, text: &str, entity: &MessageEntity) {
    if entity.type_field != MessageEntityType::CustomEmoji {
        return;
    }
    let Some(id) = entity.custom_emoji_id.as_deref() else {
        return;
    };
    let fallback = slice_utf16(text, entity.offset, entity.length);
    if fallback.is_empty() {
        return;
    }
    out.push(PendingEmoji {
        custom_emoji_id: id.to_string(),
        fallback,
    });
}

fn slice_utf16(text: &str, offset: u16, length: u16) -> String {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    let start = offset as usize;
    let end = (offset as usize + length as usize).min(utf16.len());
    if start >= utf16.len() {
        return String::new();
    }
    String::from_utf16_lossy(&utf16[start..end])
}

async fn send_all_duplicate_message(
    api: &Bot,
    chat_id: i64,
    duplicates: &[PendingEmoji],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut rendered = String::new();
    for d in duplicates {
        rendered.push_str(&format!(
            "![{}](tg://emoji?id={})",
            d.fallback, d.custom_emoji_id
        ));
    }
    let prefix = youtube::escape_markdown_v2("⚠️ همه‌ی ایموجی‌های ");
    let suffix = youtube::escape_markdown_v2(" از قبل توی دیتابیس ذخیره‌اند. چیزی به لیست اضافه نشد.");
    let text = format!("{prefix}{rendered}{suffix}");
    send_text_md(api, chat_id, &text).await
}

async fn filter_duplicates(
    client: &tokio_postgres::Client,
    owner: i64,
    incoming: &mut Vec<PendingEmoji>,
    pending: &[PendingEmoji],
) -> Vec<PendingEmoji> {
    let ids: Vec<String> = incoming.iter().map(|e| e.custom_emoji_id.clone()).collect();
    let db_dupes: std::collections::HashSet<String> =
        emoji_store::existing_custom_emoji_ids(client, owner, &ids)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
    let pending_ids: std::collections::HashSet<&str> =
        pending.iter().map(|e| e.custom_emoji_id.as_str()).collect();

    let mut duplicates = Vec::new();
    let mut kept = Vec::with_capacity(incoming.len());
    let mut seen_in_batch: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut reported_dups: std::collections::HashSet<String> = std::collections::HashSet::new();
    for emoji in incoming.drain(..) {
        let is_dup = db_dupes.contains(&emoji.custom_emoji_id)
            || pending_ids.contains(emoji.custom_emoji_id.as_str())
            || seen_in_batch.contains(&emoji.custom_emoji_id);
        if is_dup {
            if reported_dups.insert(emoji.custom_emoji_id.clone()) {
                duplicates.push(emoji);
            }
        } else {
            seen_in_batch.insert(emoji.custom_emoji_id.clone());
            kept.push(emoji);
        }
    }
    *incoming = kept;
    duplicates
}

fn apply_edit_ops(collected: &mut Vec<PendingEmoji>, text: &str) -> Result<(), &'static str> {
    let mut plus: Vec<usize> = Vec::new();
    let mut minus: Vec<usize> = Vec::new();
    for token in text.split_whitespace() {
        if let Some(rest) = token.strip_prefix('+') {
            if let Ok(idx) = rest.parse::<usize>() {
                plus.push(idx);
                continue;
            }
        }
        if let Some(rest) = token.strip_prefix('-') {
            if let Ok(idx) = rest.parse::<usize>() {
                minus.push(idx);
                continue;
            }
        }
    }
    if !plus.is_empty() && !minus.is_empty() {
        return Err("mixed");
    }
    if !plus.is_empty() {
        let snapshot = collected.clone();
        collected.clear();
        for idx in plus {
            if idx >= 1 && idx <= snapshot.len() {
                let candidate = snapshot[idx - 1].clone();
                if !collected.iter().any(|e| e.custom_emoji_id == candidate.custom_emoji_id) {
                    collected.push(candidate);
                }
            }
        }
    } else if !minus.is_empty() {
        let mut to_remove: Vec<usize> = minus
            .into_iter()
            .filter(|i| *i >= 1 && *i <= collected.len())
            .map(|i| i - 1)
            .collect();
        to_remove.sort_unstable();
        to_remove.dedup();
        for idx in to_remove.into_iter().rev() {
            collected.remove(idx);
        }
    }
    Ok(())
}
