use std::sync::Arc;
use std::time::Duration;

mod bot;
mod config;
mod cookie_pool;
mod database;
mod emoji;
mod gemini_watermark;
mod i18n;
mod modules;
mod youtube;
mod stt;
mod denoise;
mod upscale;
mod separation;

use bot::{send_text, send_start_menu, edit_to_start_menu, edit_to_ai_lab, CB_START_EMOJI, CB_START_YOUTUBE, CB_START_AI_LAB, CB_AI_DENOISE, CB_AI_UPSCALE, CB_AI_STT, CB_AI_SEP, CB_AI_GWM, CB_DENOISE_CANCEL};
use emoji::panel::CB_START_PANEL;
use stt::handle::{enter_stt_config, handle_stt_callback, handle_stt_audio};
use denoise::{enter_denoise, handle_denoise_audio};
use upscale::{enter_upscale, handle_upscale_image, handle_upscale_cancel, handle_upscale_model_pick, handle_upscale_anime_toggle, CB_UPSCALE_CANCEL, CB_UPSCALE_MODEL_PREFIX, CB_UPSCALE_ANIME_TOGGLE};
use separation::{enter_separation, handle_separation_audio, handle_separation_callback, CB_SEP_PREFIX};
use gemini_watermark::{enter_gwm, handle_gwm_image, handle_gwm_cancel, CB_GWM_CANCEL};
use stt::config::CB_STT_CANCEL;
use config::bot_token;
use cookie_pool::{CookiePool, CookieSource};
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

    // Channels for auto-refresh when a cookie is rate-limited.
    // rate_limit_tx  : handle_youtube_url → main loop, carries the CookieSource that got 429'd
    // cooldown_done_tx: refresh task → main loop, carries cookie_id to remove from cooldown
    let (rate_limit_tx, mut rate_limit_rx) = tokio::sync::mpsc::unbounded_channel::<CookieSource>();
    let (cooldown_done_tx, mut cooldown_done_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Spawn cookie refresher background task
    {
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

        let admin_chat_id = config::admin_user_id().unwrap_or(0);
        let refresh_interval_secs: u64 = config::config_value("COOKIE_REFRESH_INTERVAL_SECS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(6 * 3600);
        let refresh_api = api.clone();

        if profiles.is_empty() {
            println!("[cookie_refresher] no profiles found, skipping.");
        } else {
            println!(
                "[cookie_refresher] background task starting: {} profile(s), interval={}s",
                profiles.len(),
                refresh_interval_secs
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
                                duration_secs: 3600,
                                link_count: 3,
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
    }

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

        // Process any completed cooldown refreshes — re-add cookies to pool.
        while let Ok(cookie_id) = cooldown_done_rx.try_recv() {
            println!("[cookie_refresher] cooldown refresh done, re-adding cookie_id={cookie_id} to pool");
            cookie_pool.remove_from_cooldown(&cookie_id);
        }

        // Spawn refresh tasks for newly rate-limited cookies (fires after 30 min cooldown).
        while let Ok(source) = rate_limit_rx.try_recv() {
            let done_tx = cooldown_done_tx.clone();
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
                    duration_secs: 3600,
                    link_count: 3,
                    admin_chat_id,
                };
                if let Err(e) = modules::cookie_refresher::run(&api_clone, cfg).await {
                    eprintln!("[cookie_refresh profile={p} event=cooldown_refresh_failed] cookie_id={cookie_id} err={e}");
                } else {
                    println!("[cookie_refresh profile={p} event=cooldown_refresh_done] cookie_id={cookie_id} re-adding to pool");
                }
                // Always re-add to pool after refresh attempt (success or fail).
                let _ = done_tx.send(cookie_id);
            });
        }

        for update in updates {
            params.offset = Some(update.update_id as i64 + 1);

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

                    // /start همیشه اولویت داره — هر flow ای رو کنسل می‌کنه
                    if let (Some(uid), Some("/start")) = (user_id, message.text.as_deref()) {
                        flow_manager.clear(uid);
                        send_start_menu(&api, message.chat.id).await?;
                        continue;
                    }

                    // «لغو عملیات» از reply keyboard — فقط وقتی Idle باشه منوی استارت نشون بده
                    // (توی flow، خود flow این متن رو handle می‌کنه)
                    if let (Some(uid), Some(text)) = (user_id, message.text.as_deref()) {
                        if text.contains("لغو عملیات") && matches!(flow_manager.get(uid), FlowState::Idle) {
                            send_start_menu(&api, message.chat.id).await?;
                            continue;
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
                        // Check for STT audio state after emoji flow returned false
                        let uid = user_id.unwrap();
                        if let FlowState::AwaitingSttAudio { config } = flow_manager.get(uid) {
                            let has_audio = message.voice.is_some() || message.audio.is_some() || message.document.is_some();
                            let trace_id = next_trace_id();
                            log_trace(trace_id, "stt_route_check", &format!("user_id={uid} chat_id={} has_audio={has_audio} state=AwaitingSttAudio", message.chat.id));
                            if has_audio {
                                handle_stt_audio(&api, &message, uid, &config).await;
                                log_trace(trace_id, "stt_route_dispatched", &format!("user_id={uid} chat_id={}", message.chat.id));
                                continue;
                            }
                        }
                        if matches!(flow_manager.get(uid), FlowState::AwaitingDenoiseAudio) {
                            let has_audio = message.voice.is_some() || message.audio.is_some() || message.document.is_some();
                            let trace_id = next_trace_id();
                            log_trace(trace_id, "denoise_route_check", &format!("user_id={uid} chat_id={} has_audio={has_audio}", message.chat.id));
                            if has_audio {
                                handle_denoise_audio(&api, &message, uid, &mut flow_manager).await;
                                log_trace(trace_id, "denoise_route_dispatched", &format!("user_id={uid} chat_id={}", message.chat.id));
                                continue;
                            }
                        }
                        if let FlowState::AwaitingUpscaleImage { scale_factor, model_name, .. } = flow_manager.get(uid) {
                            let has_image = message.photo.is_some() || message.document.is_some();
                            let trace_id = next_trace_id();
                            log_trace(trace_id, "upscale_route_check", &format!(
                                "user_id={uid} chat_id={} has_image={has_image} model={model_name} scale={scale_factor}", message.chat.id
                            ));
                            if has_image {
                                handle_upscale_image(&api, &message, uid, scale_factor, &model_name, &mut flow_manager).await;
                                log_trace(trace_id, "upscale_route_dispatched", &format!("user_id={uid} chat_id={}", message.chat.id));
                                continue;
                            }
                        }
                        if matches!(flow_manager.get(uid), FlowState::AwaitingSeparation) {
                            let has_audio = message.audio.is_some() || message.voice.is_some() || message.document.is_some();
                            let trace_id = next_trace_id();
                            log_trace(trace_id, "separation_route_check", &format!(
                                "user_id={uid} chat_id={} has_audio={has_audio}", message.chat.id
                            ));
                            if has_audio {
                                handle_separation_audio(&api, &message, uid, &mut flow_manager).await;
                                log_trace(trace_id, "separation_route_dispatched", &format!("user_id={uid} chat_id={}", message.chat.id));
                                continue;
                            }
                        }
                        if matches!(flow_manager.get(uid), FlowState::AwaitingGeminiWmImage) {
                            let has_image = message.photo.is_some() || message.document.is_some();
                            let trace_id = next_trace_id();
                            log_trace(trace_id, "gwm_route_check", &format!(
                                "user_id={uid} chat_id={} has_image={has_image}", message.chat.id
                            ));
                            if has_image {
                                handle_gwm_image(&api, &message, uid, &mut flow_manager).await;
                                log_trace(trace_id, "gwm_route_dispatched", &format!("user_id={uid} chat_id={}", message.chat.id));
                                continue;
                            }
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
                                    handle_youtube_url(&api, message.chat.id, message.message_id, user_id, trace_id, &url, &mut cookie_pool, &database, &rate_limit_tx).await;
                                }
                            }
                        }
                    }
                }
                UpdateContent::CallbackQuery(callback_query) => {
                    let cb_user_id = callback_query.from.id;
                    let cb_data = callback_query.data.as_deref().unwrap_or("");
                    let cb_chat_id = callback_query.message.as_ref().and_then(|m| match m {
                        MaybeInaccessibleMessage::Message(msg) => Some(msg.chat.id),
                        _ => None,
                    }).unwrap_or(0);
                    eprintln!(
                        "[main event=callback_received] user_id={cb_user_id} chat_id={cb_chat_id} data={cb_data:?}"
                    );
                    if callback_query.data.as_deref().map(|d| d.starts_with("emoji:")).unwrap_or(false) {
                        emoji_handler::handle_emoji_callback(&api, &callback_query, &mut flow_manager, &database).await;
                        continue;
                    }
                    if handle_quality_callback(&api, &callback_query).await {
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_START_EMOJI) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "cb_start_emoji", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
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
                        log_trace(trace_id, "cb_start_emoji_done", "");
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_START_YOUTUBE) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "cb_start_youtube", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
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
                            let r = api.send_message(&params).await;
                            log_trace(trace_id, "cb_start_youtube_sent", &format!("ok={}", r.is_ok()));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_START_PANEL) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "cb_start_panel", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            let r = edit_to_start_menu(&api, message.chat.id, message.message_id).await;
                            log_trace(trace_id, "cb_start_panel_done", &format!("ok={}", r.is_ok()));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_START_AI_LAB) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "cb_start_ai_lab", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            let r = edit_to_ai_lab(&api, message.chat.id, message.message_id).await;
                            log_trace(trace_id, "cb_start_ai_lab_done", &format!("ok={}", r.is_ok()));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_AI_DENOISE) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "cb_ai_denoise_entry", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            enter_denoise(
                                &api, message.chat.id, message.message_id,
                                cb_user_id as i64, &mut flow_manager,
                            ).await;
                            log_trace(trace_id, "cb_ai_denoise_done", &format!("user_id={cb_user_id} chat_id={}", message.chat.id));
                        } else {
                            log_trace(trace_id, "cb_ai_denoise_no_message", &format!("user_id={cb_user_id}"));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_AI_UPSCALE) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "cb_ai_upscale_entry", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            enter_upscale(
                                &api, message.chat.id, message.message_id,
                                cb_user_id as i64, &mut flow_manager,
                            ).await;
                            log_trace(trace_id, "cb_ai_upscale_done", &format!("user_id={cb_user_id} chat_id={}", message.chat.id));
                        } else {
                            log_trace(trace_id, "cb_ai_upscale_no_message", &format!("user_id={cb_user_id}"));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_AI_STT) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "cb_ai_stt_entry", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            enter_stt_config(
                                &api, message.chat.id, message.message_id,
                                cb_user_id as i64, &mut flow_manager,
                            ).await;
                            log_trace(trace_id, "cb_ai_stt_done", &format!("user_id={cb_user_id} chat_id={}", message.chat.id));
                        } else {
                            log_trace(trace_id, "cb_ai_stt_no_message", &format!("user_id={cb_user_id}"));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_DENOISE_CANCEL) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "denoise_cancel", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            denoise::handle_denoise_cancel(
                                &api, message.chat.id, message.message_id,
                                cb_user_id as i64, &mut flow_manager,
                            ).await;
                            log_trace(trace_id, "denoise_cancel_done", &format!("user_id={cb_user_id} chat_id={}", message.chat.id));
                        } else {
                            log_trace(trace_id, "denoise_cancel_no_message", &format!("user_id={cb_user_id}"));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_UPSCALE_CANCEL) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "upscale_cancel", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            handle_upscale_cancel(
                                &api, message.chat.id, message.message_id,
                                cb_user_id as i64, &mut flow_manager,
                            ).await;
                            log_trace(trace_id, "upscale_cancel_done", &format!("user_id={cb_user_id} chat_id={}", message.chat.id));
                        } else {
                            log_trace(trace_id, "upscale_cancel_no_message", &format!("user_id={cb_user_id}"));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_UPSCALE_ANIME_TOGGLE) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "upscale_anime_toggle", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            handle_upscale_anime_toggle(
                                &api, message.chat.id, message.message_id,
                                cb_user_id as i64, &mut flow_manager,
                            ).await;
                        }
                        continue;
                    }
                    if callback_query.data.as_deref().map(|d| d.starts_with(CB_UPSCALE_MODEL_PREFIX)).unwrap_or(false) {
                        let trace_id = next_trace_id();
                        let model_name = cb_data.strip_prefix(CB_UPSCALE_MODEL_PREFIX).unwrap_or("");
                        log_trace(trace_id, "upscale_model_pick", &format!("user_id={cb_user_id} model={model_name}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            handle_upscale_model_pick(
                                &api, model_name, message.chat.id, message.message_id,
                                cb_user_id as i64, &mut flow_manager,
                            ).await;
                            log_trace(trace_id, "upscale_model_pick_done", &format!("user_id={cb_user_id} model={model_name}"));
                        } else {
                            log_trace(trace_id, "upscale_model_pick_no_message", &format!("user_id={cb_user_id} model={model_name}"));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref().map(|d| d.starts_with("stt:")).unwrap_or(false) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "stt_callback", &format!("user_id={cb_user_id} data={cb_data:?}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            handle_stt_callback(
                                &api, cb_data, message.chat.id, message.message_id,
                                cb_user_id as i64, &mut flow_manager,
                            ).await;
                            log_trace(trace_id, "stt_callback_done", &format!("user_id={cb_user_id} data={cb_data:?}"));
                        } else {
                            log_trace(trace_id, "stt_callback_no_message", &format!("user_id={cb_user_id} data={cb_data:?}"));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_AI_SEP) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "cb_ai_sep_entry", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            enter_separation(
                                &api, message.chat.id, message.message_id,
                                cb_user_id as i64, &mut flow_manager,
                            ).await;
                            log_trace(trace_id, "cb_ai_sep_done", &format!("user_id={cb_user_id} chat_id={}", message.chat.id));
                        } else {
                            log_trace(trace_id, "cb_ai_sep_no_message", &format!("user_id={cb_user_id}"));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_AI_GWM) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "cb_ai_gwm_entry", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            enter_gwm(
                                &api, message.chat.id, message.message_id,
                                cb_user_id as i64, &mut flow_manager,
                            ).await;
                            log_trace(trace_id, "cb_ai_gwm_done", &format!("user_id={cb_user_id} chat_id={}", message.chat.id));
                        } else {
                            log_trace(trace_id, "cb_ai_gwm_no_message", &format!("user_id={cb_user_id}"));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref() == Some(CB_GWM_CANCEL) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "gwm_cancel", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            handle_gwm_cancel(
                                &api, message.chat.id, message.message_id,
                                cb_user_id as i64, &mut flow_manager,
                            ).await;
                            log_trace(trace_id, "gwm_cancel_done", &format!("user_id={cb_user_id} chat_id={}", message.chat.id));
                        } else {
                            log_trace(trace_id, "gwm_cancel_no_message", &format!("user_id={cb_user_id}"));
                        }
                        continue;
                    }
                    if callback_query.data.as_deref().map(|d| d.starts_with(CB_SEP_PREFIX)).unwrap_or(false) {
                        let trace_id = next_trace_id();
                        log_trace(trace_id, "sep_callback", &format!("user_id={cb_user_id} data={cb_data:?}"));
                        let _ = api.answer_callback_query(
                            &AnswerCallbackQueryParams::builder()
                                .callback_query_id(callback_query.id)
                                .build(),
                        ).await;
                        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                            handle_separation_callback(
                                &api, cb_data, message.chat.id, message.message_id,
                                cb_user_id as i64, &mut flow_manager,
                            ).await;
                            log_trace(trace_id, "sep_callback_done", &format!("user_id={cb_user_id} data={cb_data:?}"));
                        } else {
                            log_trace(trace_id, "sep_callback_no_message", &format!("user_id={cb_user_id} data={cb_data:?}"));
                        }
                        continue;
                    }
                    // هر callback ناشناخته (مثلاً دکمه‌های قدیمی بعد از ری‌استارت) → منوی استارت
                    eprintln!(
                        "[main event=callback_unhandled] user_id={cb_user_id} chat_id={cb_chat_id} data={cb_data:?} — sending start menu"
                    );
                    let _ = api.answer_callback_query(
                        &AnswerCallbackQueryParams::builder()
                            .callback_query_id(callback_query.id)
                            .build(),
                    ).await;
                    if cb_chat_id != 0 {
                        flow_manager.clear(cb_user_id as i64);
                        let _ = send_start_menu(&api, cb_chat_id).await;
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
