use std::{collections::HashSet, fs};
use chrono::{Datelike, Timelike, TimeZone};
use frankenstein::methods::GetFileParams;
use chrono_tz::Asia::Tehran;
use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{
        AnswerCallbackQueryParams, EditMessageTextParams, GetCustomEmojiStickersParams,
        GetStickerSetParams, SendDocumentParams, SendMessageParams,
    },
    types::{
        InlineKeyboardMarkup, LinkPreviewOptions, MaybeInaccessibleMessage, Message,
        MessageEntity, MessageEntityType, ReplyMarkup, ReplyKeyboardRemove,
    },
};

use crate::bot::{send_text, send_text_md};
use crate::database::postgresql::PostgresDatabase;
use crate::i18n::{entities_for_text, t, tf};
use super::{
    FlowManager, FlowState, PendingEmoji,
    import as emoji_import,
    panel::{self as emoji_panel, CB_ADD, CB_BACK, CB_BACK_TO_PACK_CHOICE, CB_CANCEL,
            CB_DELETE_PACK_MENU, CB_EXPORT,
            CB_IMPORT, CB_IMPORT_MERGE, CB_IMPORT_REPLACE, CB_IMPORT_SMART,
            CB_LIST, CB_PACKS, CB_PACK_DELETE_PREFIX, CB_PACK_OPEN_PREFIX,
            CB_PENDING_PAGE_PREFIX, CB_PICK_PACK_PREFIX,
            CB_PACK_SET_ALIAS_PREFIX, CB_PACK_SET_DEFAULT_PREFIX,
            CB_SHOW_PACK_LINKS, CB_TEST},
    store as emoji_store,
};

pub async fn handle_emoji_command(
    api: &Bot,
    message: &Message,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let chat_id = message.chat.id;
    let user_id = match message.from.as_ref() {
        Some(u) => u.id as i64,
        None => return,
    };
    flow_manager.clear(user_id);
    if database.is_none() {
        let _ = send_text(api, chat_id, &t("emoji.db_required")).await;
        return;
    }
    let panel_text = emoji_panel::main_panel_text();
    let ents = entities_for_text(&panel_text);
    let params = if ents.is_empty() {
        SendMessageParams::builder().chat_id(chat_id).text(panel_text)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard())).build()
    } else {
        SendMessageParams::builder().chat_id(chat_id).text(panel_text).entities(ents)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard())).build()
    };
    let _ = api.send_message(&params).await;
}

pub async fn handle_emoji_callback(
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

    let Some(data) = cbq.data.as_deref() else { return };
    let Some(MaybeInaccessibleMessage::Message(panel_msg)) = cbq.message.clone() else { return };
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
            flow_manager.set(user_id, FlowState::AwaitingEmojis { collected: Vec::new() });
            send_with_ents(api, chat_id, t("emoji.add_prompt"),
                Some(ReplyMarkup::ReplyKeyboardMarkup(emoji_panel::cancel_reply_keyboard()))).await;
        }
        d if d == CB_TEST => {
            flow_manager.set(user_id, FlowState::AwaitingTestText);
            send_with_ents(api, chat_id, t("emoji.test_prompt"),
                Some(ReplyMarkup::ReplyKeyboardMarkup(emoji_panel::cancel_reply_keyboard()))).await;
        }
        d if d == CB_LIST => {
            send_emoji_list(api, chat_id, user_id, client).await;
        }
        d if d == CB_PACKS || d == CB_DELETE_PACK_MENU => {
            show_packs_menu(api, chat_id, message_id, user_id, client).await;
        }
        d if d == CB_IMPORT => {
            flow_manager.set(user_id, FlowState::AwaitingImportFile);
            send_with_ents(api, chat_id, t("emoji.import_prompt"),
                Some(ReplyMarkup::ReplyKeyboardMarkup(emoji_panel::cancel_reply_keyboard()))).await;
        }
        d if d == CB_EXPORT => {
            match emoji_store::export_user_sql(client, user_id).await {
                Err(e) => {
                    eprintln!("export_user_sql failed: {e}");
                    let _ = send_text(api, chat_id, &t("emoji.export_failed")).await;
                }
                Ok(sql) => {
                    let now = chrono::Utc::now().with_timezone(&Tehran);
                    let (jy, jm, jd) = gregorian_to_jalali(now.year(), now.month() as u32, now.day());
                    let filename = format!(
                        "emoji_{:04}-{:02}-{:02}_{:02}-{:02}.sql",
                        jy, jm, jd, now.hour(), now.minute(),
                    );
                    let path = std::env::temp_dir().join(&filename);
                    if let Err(e) = fs::write(&path, &sql) {
                        eprintln!("write export file failed: {e}");
                        let _ = send_text(api, chat_id, &t("emoji.export_failed")).await;
                    } else {
                        let _ = api.send_document(
                            &SendDocumentParams::builder()
                                .chat_id(chat_id)
                                .document(FileUpload::InputFile(InputFile { path: path.clone() }))
                                .caption(t("emoji.export_caption"))
                                .build(),
                        ).await;
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }
        d if d == CB_BACK || d == CB_CANCEL => {
            flow_manager.clear(user_id);
            edit_panel(api, chat_id, message_id, &emoji_panel::main_panel_text(), Some(emoji_panel::main_panel_keyboard())).await;
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
            if let Some(pack_id) = d.strip_prefix(CB_PACK_OPEN_PREFIX).and_then(|s| s.parse::<i32>().ok()) {
                show_pack_detail(api, chat_id, message_id, user_id, pack_id, client).await;
            }
        }
        d if d.starts_with(CB_PACK_SET_DEFAULT_PREFIX) => {
            if let Some(pack_id) = d.strip_prefix(CB_PACK_SET_DEFAULT_PREFIX).and_then(|s| s.parse::<i32>().ok()) {
                if let Err(e) = emoji_store::set_default_pack(client, user_id, pack_id).await {
                    eprintln!("set_default_pack failed: {e}");
                }
                show_pack_detail(api, chat_id, message_id, user_id, pack_id, client).await;
            }
        }
        d if d.starts_with(CB_PACK_SET_ALIAS_PREFIX) => {
            if let Some(pack_id) = d.strip_prefix(CB_PACK_SET_ALIAS_PREFIX).and_then(|s| s.parse::<i32>().ok()) {
                flow_manager.set(user_id, FlowState::AwaitingPackAlias { pack_id });
                let _ = send_text(api, chat_id, &t("emoji.pack_alias_prompt")).await;
            }
        }
        d if d.starts_with(CB_PACK_DELETE_PREFIX) => {
            if let Some(pack_id) = d.strip_prefix(CB_PACK_DELETE_PREFIX).and_then(|s| s.parse::<i32>().ok()) {
                let name = emoji_store::list_packs(client, user_id)
                    .await
                    .ok()
                    .and_then(|packs| packs.into_iter().find(|p| p.id == pack_id))
                    .map(|p| p.name)
                    .unwrap_or_default();
                if let Err(e) = emoji_store::delete_pack(client, user_id, pack_id).await {
                    eprintln!("delete_pack failed: {e}");
                }
                {
                    let msg = tf("emoji.pack_deleted", &[("name", &name)]);
                    let ents = entities_for_text(&msg);
                    let no_preview = LinkPreviewOptions::builder().is_disabled(true).build();
                    let params = if ents.is_empty() {
                        SendMessageParams::builder().chat_id(chat_id).text(&msg)
                            .link_preview_options(no_preview).build()
                    } else {
                        SendMessageParams::builder().chat_id(chat_id).text(&msg)
                            .entities(ents).link_preview_options(no_preview).build()
                    };
                    let _ = api.send_message(&params).await;
                }
                send_with_ents(api, chat_id, emoji_panel::main_panel_text(),
                    Some(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard()))).await;
            }
        }
        d if d == CB_SHOW_PACK_LINKS => {
            if let FlowState::AwaitingPackChoice { collected } = flow_manager.get(user_id) {
                let ids: Vec<String> = collected.iter()
                    .map(|e| e.custom_emoji_id.clone())
                    .collect();
                let text = build_pack_links_text(api, &collected, &ids).await;
                let no_preview = LinkPreviewOptions::builder().is_disabled(true).build();
                let params = EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(message_id)
                    .text(&text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .link_preview_options(no_preview)
                    .reply_markup(emoji_panel::pack_links_keyboard())
                    .build();
                if let Err(e) = api.edit_message_text(&params).await {
                    eprintln!("edit pack links failed: {e}");
                }
            }
        }
        d if d == CB_BACK_TO_PACK_CHOICE => {
            if let FlowState::AwaitingPackChoice { collected } = flow_manager.get(user_id) {
                let total_pages = emoji_panel::pending_total_pages(collected.len());
                let summary = emoji_panel::format_pending_emojis(&collected, &[], 0);
                let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
                let params = EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(message_id)
                    .text(&summary)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages))
                    .build();
                if let Err(e) = api.edit_message_text(&params).await {
                    eprintln!("edit back to pack choice failed: {e}");
                }
            }
        }
        d if d.starts_with(CB_PENDING_PAGE_PREFIX) => {
            if let Some(page) = d
                .strip_prefix(CB_PENDING_PAGE_PREFIX)
                .and_then(|s| s.parse::<usize>().ok())
            {
                if let FlowState::AwaitingPackChoice { collected } = flow_manager.get(user_id) {
                    let total_pages = emoji_panel::pending_total_pages(collected.len());
                    let text = emoji_panel::format_pending_emojis(&collected, &[], page);
                    let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
                    let params = EditMessageTextParams::builder()
                        .chat_id(chat_id)
                        .message_id(message_id)
                        .text(&text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(emoji_panel::pack_choice_keyboard(&packs, page, total_pages))
                        .build();
                    if let Err(e) = api.edit_message_text(&params).await {
                        eprintln!("edit pending page failed: {e}");
                    }
                }
            }
        }
        d if d.starts_with(CB_PICK_PACK_PREFIX) => {
            if let Some(pack_id) = d.strip_prefix(CB_PICK_PACK_PREFIX).and_then(|s| s.parse::<i32>().ok()) {
                if let FlowState::AwaitingPackChoice { collected } = flow_manager.get(user_id) {
                    let collected = collected.clone();
                    flow_manager.clear(user_id);
                    add_emojis_to_pack(api, chat_id, &collected, pack_id, user_id, client).await;
                }
            }
        }
        d if d == CB_IMPORT_REPLACE || d == CB_IMPORT_MERGE || d == CB_IMPORT_SMART => {
            let sql = match flow_manager.get(user_id) {
                FlowState::AwaitingImportMode { sql } => sql,
                _ => return,
            };
            flow_manager.clear(user_id);
            let parsed = emoji_import::parse_sql(&sql);
            let result = if d == CB_IMPORT_REPLACE {
                emoji_import::execute_replace(&parsed, client, user_id).await
            } else {
                emoji_import::execute_merge(&parsed, client, user_id, d == CB_IMPORT_SMART).await
            };
            match result {
                Ok(r) => {
                    let _ = send_text(api, chat_id, &tf("emoji.import_result", &[
                        ("packs", &r.packs_added.to_string()),
                        ("items", &r.items_added.to_string()),
                        ("skipped", &r.items_skipped.to_string()),
                    ])).await;
                }
                Err(e) => {
                    eprintln!("import execute failed: {e}");
                    let _ = send_text(api, chat_id, &t("emoji.import_failed")).await;
                }
            }
            send_with_ents(api, chat_id, emoji_panel::main_panel_text(),
                Some(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard()))).await;
        }
        _ => {}
    }
}

pub async fn handle_emoji_flow_message(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) -> bool {
    let chat_id = message.chat.id;
    let Some(db) = database else { return false };
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
            // 19-digit number → treat as custom emoji ID
            let id_hits = extract_19digit_ids(msg_text);
            if !id_hits.is_empty() {
                let stickers = api.get_custom_emoji_stickers(
                    &GetCustomEmojiStickersParams::builder()
                        .custom_emoji_ids(id_hits)
                        .build(),
                ).await.map(|r| r.result).unwrap_or_default();
                let mut from_ids: Vec<PendingEmoji> = stickers.into_iter()
                    .filter_map(|s| Some(PendingEmoji {
                        custom_emoji_id: s.custom_emoji_id?,
                        fallback: s.emoji.unwrap_or_else(|| "?".to_string()),
                    }))
                    .collect();
                let incoming = from_ids.len();
                let duplicates = filter_duplicates(client, user_id, &mut from_ids, &collected).await;
                if incoming > 0 && from_ids.is_empty() && collected.is_empty() {
                    let _ = send_all_duplicate_message(api, chat_id, &duplicates).await;
                    flow_manager.set(user_id, FlowState::AwaitingEmojis { collected });
                    return true;
                }
                collected.extend(from_ids);
                let total_pages = emoji_panel::pending_total_pages(collected.len());
                let text = emoji_panel::format_pending_emojis(&collected, &duplicates, 0);
                let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
                let _ = api.send_message(
                    &SendMessageParams::builder()
                        .chat_id(chat_id)
                        .text(text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages)))
                        .build(),
                ).await;
                flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
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
            let total_pages = emoji_panel::pending_total_pages(collected.len());
            let text = emoji_panel::format_pending_emojis(&collected, &duplicates, 0);
            let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
            let _ = api.send_message(
                &SendMessageParams::builder()
                    .chat_id(chat_id)
                    .text(text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages)))
                    .build(),
            ).await;
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
                let duplicates = filter_duplicates(client, user_id, &mut extras, &collected).await;
                if incoming > 0 && extras.is_empty() {
                    let _ = send_all_duplicate_message(api, chat_id, &duplicates).await;
                    flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
                    return true;
                }
                collected.extend(extras);
                let total_pages = emoji_panel::pending_total_pages(collected.len());
                let summary = emoji_panel::format_pending_emojis(&collected, &duplicates, 0);
                let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
                let _ = api.send_message(
                    &SendMessageParams::builder()
                        .chat_id(chat_id)
                        .text(summary)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages)))
                        .build(),
                ).await;
                flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
                return true;
            }
            if text.starts_with('-') || text.starts_with('+') {
                if apply_edit_ops(&mut collected, text).is_err() {
                    let _ = send_text(api, chat_id, &t("emoji.pending.mixed_ops")).await;
                    flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
                    return true;
                }
                let total_pages = emoji_panel::pending_total_pages(collected.len());
                let summary = emoji_panel::format_pending_emojis(&collected, &[], 0);
                let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
                let _ = api.send_message(
                    &SendMessageParams::builder()
                        .chat_id(chat_id)
                        .text(summary)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages)))
                        .build(),
                ).await;
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
                    Err(e) => { eprintln!("create_pack failed: {e}"); flow_manager.clear(user_id); return true; }
                },
                Err(e) => { eprintln!("find_pack_by_name failed: {e}"); flow_manager.clear(user_id); return true; }
            };
            let mut added = 0;
            for emoji in &collected {
                let smart = match emoji_store::allocate_smart_name(client, user_id, &emoji.fallback).await {
                    Ok(s) => s,
                    Err(e) => { eprintln!("allocate_smart_name failed: {e}"); continue; }
                };
                if let Err(e) = emoji_store::add_item(client, user_id, pack.id, &emoji.custom_emoji_id, &emoji.fallback, &smart).await {
                    eprintln!("add_item failed: {e}"); continue;
                }
                added += 1;
            }
            send_with_ents(api, chat_id,
                tf("emoji.added_summary", &[("count", &added.to_string()), ("pack", &pack.name)]),
                Some(ReplyMarkup::ReplyKeyboardRemove(ReplyKeyboardRemove::builder().remove_keyboard(true).build()))).await;
            send_with_ents(api, chat_id, emoji_panel::main_panel_text(),
                Some(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard()))).await;
            flow_manager.clear(user_id);
            true
        }
        FlowState::AwaitingPackAlias { pack_id } => {
            let text = message.text.as_deref().unwrap_or("").trim();
            let alias = if text == "-" || text.is_empty() { None } else { Some(text) };
            if let Err(e) = emoji_store::set_pack_alias(client, user_id, pack_id, alias).await {
                eprintln!("set_pack_alias failed: {e}");
            }
            let _ = send_text(api, chat_id, &t("emoji.pack_alias_set")).await;
            flow_manager.clear(user_id);
            true
        }
        FlowState::AwaitingImportFile => {
            let text = message.text.as_deref().unwrap_or("").trim();
            if text == t("emoji.cancel_button") {
                flow_manager.clear(user_id);
                send_cancel_and_panel(api, chat_id).await;
                return true;
            }
            let Some(doc) = message.document.as_ref() else {
                let _ = send_text(api, chat_id, &t("emoji.import_send_file")).await;
                return true;
            };
            let file_id = doc.file_id.clone();
            let token = match crate::config::config_value("BOT_TOKEN") {
                Some(t) => t,
                None => { flow_manager.clear(user_id); return true; }
            };
            let file_path = match api.get_file(&GetFileParams::builder().file_id(file_id).build()).await {
                Ok(r) => match r.result.file_path {
                    Some(p) => p,
                    None => {
                        let _ = send_text(api, chat_id, &t("emoji.import_failed")).await;
                        return true;
                    }
                },
                Err(e) => {
                    eprintln!("get_file failed: {e}");
                    let _ = send_text(api, chat_id, &t("emoji.import_failed")).await;
                    return true;
                }
            };
            let url = format!("https://api.telegram.org/file/bot{token}/{file_path}");
            let sql = match reqwest::get(&url).await {
                Ok(resp) => match resp.text().await {
                    Ok(t) => t,
                    Err(e) => { eprintln!("read import body failed: {e}"); let _ = send_text(api, chat_id, &t("emoji.import_failed")).await; return true; }
                },
                Err(e) => { eprintln!("download import file failed: {e}"); let _ = send_text(api, chat_id, &t("emoji.import_failed")).await; return true; }
            };
            let parsed = emoji_import::parse_sql(&sql);
            if parsed.packs.is_empty() && parsed.items.is_empty() {
                let _ = send_text(api, chat_id, &t("emoji.import_empty_file")).await;
                flow_manager.clear(user_id);
                return true;
            }
            let analysis = emoji_import::analyze(&parsed, client, user_id).await;
            let report = build_import_report(&analysis);
            let keyboard = emoji_panel::import_choice_keyboard(analysis.db_empty);
            send_with_ents(api, chat_id, report,
                Some(ReplyMarkup::InlineKeyboardMarkup(keyboard))).await;
            flow_manager.set(user_id, FlowState::AwaitingImportMode { sql });
            true
        }
        FlowState::AwaitingImportMode { .. } => {
            let text = message.text.as_deref().unwrap_or("").trim();
            if text == t("emoji.cancel_button") {
                flow_manager.clear(user_id);
                send_cancel_and_panel(api, chat_id).await;
            }
            true
        }
        FlowState::AwaitingTestText => {
            let text = message.text.as_deref().unwrap_or("").trim();
            if text == t("emoji.cancel_button") {
                flow_manager.clear(user_id);
                send_cancel_and_panel(api, chat_id).await;
                return true;
            }
            let rendered = if let Some(cache_arc) = super::cache::global() {
                let cache = cache_arc.read().await;
                cache.render_markdown(text)
            } else {
                text.to_string()
            };
            let _ = api.send_message(
                &SendMessageParams::builder()
                    .chat_id(chat_id)
                    .text(rendered)
                    .parse_mode(ParseMode::MarkdownV2)
                    .build(),
            ).await;
            true
        }
    }
}

pub async fn handle_se_command(
    api: &Bot,
    message: &Message,
    rest: &str,
    database: &Option<PostgresDatabase>,
) {
    let chat_id = message.chat.id;
    let Some(user) = message.from.as_ref() else { return };
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
        Ok(true) => { let _ = send_text(api, chat_id, &tf("emoji.se_done", &[("alias", alias)])).await; }
        Ok(false) => { let _ = send_text(api, chat_id, &t("emoji.se_not_found")).await; }
        Err(e) => { eprintln!("set_item_alias failed: {e}"); }
    }
}

async fn send_cancel_and_panel(api: &Bot, chat_id: i64) {
    let _ = api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(t("emoji.canceled"))
            .reply_markup(ReplyMarkup::ReplyKeyboardRemove(ReplyKeyboardRemove::builder().remove_keyboard(true).build()))
            .build(),
    ).await;
    let _ = api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(emoji_panel::main_panel_text())
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard()))
            .build(),
    ).await;
}

async fn add_emojis_to_pack(
    api: &Bot,
    chat_id: i64,
    collected: &[PendingEmoji],
    pack_id: i32,
    user_id: i64,
    client: &tokio_postgres::Client,
) {
    let pack_name = emoji_store::list_packs(client, user_id)
        .await
        .ok()
        .and_then(|packs| packs.into_iter().find(|p| p.id == pack_id).map(|p| p.name))
        .unwrap_or_else(|| pack_id.to_string());
    let mut added = 0;
    for emoji in collected {
        let smart = match emoji_store::allocate_smart_name(client, user_id, &emoji.fallback).await {
            Ok(s) => s,
            Err(e) => { eprintln!("allocate_smart_name failed: {e}"); continue; }
        };
        if let Err(e) = emoji_store::add_item(client, user_id, pack_id, &emoji.custom_emoji_id, &emoji.fallback, &smart).await {
            eprintln!("add_item failed: {e}"); continue;
        }
        added += 1;
    }
    let _ = api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(tf("emoji.added_summary", &[("count", &added.to_string()), ("pack", &pack_name)]))
            .reply_markup(ReplyMarkup::ReplyKeyboardRemove(ReplyKeyboardRemove::builder().remove_keyboard(true).build()))
            .build(),
    ).await;
    let _ = api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(emoji_panel::main_panel_text())
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard()))
            .build(),
    ).await;
}

async fn send_emoji_list(api: &Bot, chat_id: i64, user_id: i64, client: &tokio_postgres::Client) {
    let packs = match emoji_store::list_packs(client, user_id).await {
        Ok(p) => p,
        Err(e) => { eprintln!("list_packs failed: {e}"); return; }
    };
    if packs.is_empty() {
        let keyboard = InlineKeyboardMarkup::builder()
            .inline_keyboard(vec![vec![emoji_panel::btn(
                &t("emoji.panel.back"),
                emoji_panel::CB_BACK,
            )]])
            .build();
        let _ = api
            .send_message(
                &SendMessageParams::builder()
                    .chat_id(chat_id)
                    .text(t("emoji.no_packs"))
                    .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard))
                    .build(),
            )
            .await;
        return;
    }
    let mut packs_with_items = Vec::new();
    for pack in packs {
        let items = emoji_store::list_items(client, pack.id).await.unwrap_or_default();
        packs_with_items.push((pack, items));
    }
    let (text, page, total_pages) = emoji_panel::build_list_page(&packs_with_items, 0);
    let keyboard = emoji_panel::list_page_keyboard(page, total_pages);
    let no_preview = LinkPreviewOptions::builder().is_disabled(true).build();
    let _ = api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .parse_mode(ParseMode::MarkdownV2)
            .link_preview_options(no_preview)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard))
            .build(),
    ).await;
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
        Err(e) => { eprintln!("list_packs failed: {e}"); return; }
    };
    let mut packs_with_items = Vec::new();
    for pack in packs {
        let items = emoji_store::list_items(client, pack.id).await.unwrap_or_default();
        packs_with_items.push((pack, items));
    }
    let (text, page, total_pages) = emoji_panel::build_list_page(&packs_with_items, page);
    let keyboard = emoji_panel::list_page_keyboard(page, total_pages);
    let no_preview = LinkPreviewOptions::builder().is_disabled(true).build();
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .parse_mode(ParseMode::MarkdownV2)
        .link_preview_options(no_preview)
        .reply_markup(keyboard)
        .build();
    if let Err(e) = api.edit_message_text(&params).await {
        eprintln!("edit_message_text failed: {e}");
    }
}

async fn show_packs_menu(api: &Bot, chat_id: i64, message_id: i32, user_id: i64, client: &tokio_postgres::Client) {
    let packs = match emoji_store::list_packs(client, user_id).await {
        Ok(p) => p,
        Err(e) => { eprintln!("list_packs failed: {e}"); return; }
    };
    if packs.is_empty() {
        let keyboard = InlineKeyboardMarkup::builder()
            .inline_keyboard(vec![vec![emoji_panel::btn(
                &t("emoji.panel.back"),
                emoji_panel::CB_BACK,
            )]])
            .build();
        let _ = api
            .send_message(
                &SendMessageParams::builder()
                    .chat_id(chat_id)
                    .text(t("emoji.no_packs"))
                    .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard))
                    .build(),
            )
            .await;
        return;
    }
    edit_panel(api, chat_id, message_id, "📁 مجموعه‌ها:", Some(emoji_panel::packs_keyboard(&packs))).await;
}

async fn show_pack_detail(api: &Bot, chat_id: i64, message_id: i32, user_id: i64, pack_id: i32, client: &tokio_postgres::Client) {
    let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
    let Some(pack) = packs.into_iter().find(|p| p.id == pack_id) else { return };
    edit_panel(api, chat_id, message_id, &emoji_panel::pack_detail_text(&pack), Some(emoji_panel::pack_detail_keyboard(&pack))).await;
}

async fn send_with_ents(api: &Bot, chat_id: i64, text: String, reply_markup: Option<ReplyMarkup>) {
    let ents = entities_for_text(&text);
    let params = match (ents.is_empty(), reply_markup) {
        (true, None) => SendMessageParams::builder().chat_id(chat_id).text(text).build(),
        (true, Some(rm)) => SendMessageParams::builder().chat_id(chat_id).text(text).reply_markup(rm).build(),
        (false, None) => SendMessageParams::builder().chat_id(chat_id).text(text).entities(ents).build(),
        (false, Some(rm)) => SendMessageParams::builder().chat_id(chat_id).text(text).entities(ents).reply_markup(rm).build(),
    };
    let _ = api.send_message(&params).await;
}

async fn edit_panel(api: &Bot, chat_id: i64, message_id: i32, text: &str, keyboard: Option<InlineKeyboardMarkup>) {
    let ents = entities_for_text(text);
    let np = || LinkPreviewOptions::builder().is_disabled(true).build();
    let params = match (ents.is_empty(), keyboard) {
        (true, None) => EditMessageTextParams::builder()
            .chat_id(chat_id).message_id(message_id).text(text)
            .link_preview_options(np()).build(),
        (true, Some(kb)) => EditMessageTextParams::builder()
            .chat_id(chat_id).message_id(message_id).text(text)
            .link_preview_options(np()).reply_markup(kb).build(),
        (false, None) => EditMessageTextParams::builder()
            .chat_id(chat_id).message_id(message_id).text(text)
            .entities(ents).link_preview_options(np()).build(),
        (false, Some(kb)) => EditMessageTextParams::builder()
            .chat_id(chat_id).message_id(message_id).text(text)
            .entities(ents).link_preview_options(np()).reply_markup(kb).build(),
    };
    if let Err(e) = api.edit_message_text(&params).await {
        eprintln!("edit_message_text failed: {e}");
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
    if entity.type_field != MessageEntityType::CustomEmoji { return }
    let Some(id) = entity.custom_emoji_id.as_deref() else { return };
    let fallback = slice_utf16(text, entity.offset, entity.length);
    if fallback.is_empty() { return }
    out.push(PendingEmoji { custom_emoji_id: id.to_string(), fallback });
}

fn slice_utf16(text: &str, offset: u16, length: u16) -> String {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    let start = offset as usize;
    let end = (offset as usize + length as usize).min(utf16.len());
    if start >= utf16.len() { return String::new() }
    String::from_utf16_lossy(&utf16[start..end])
}

async fn send_all_duplicate_message(
    api: &Bot,
    chat_id: i64,
    duplicates: &[PendingEmoji],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut rendered = String::new();
    for d in duplicates {
        rendered.push_str(&format!("![{}](tg://emoji?id={})", d.fallback, d.custom_emoji_id));
    }
    let prefix = crate::youtube::escape_markdown_v2("⚠️ همه‌ی ایموجی‌های ");
    let suffix = crate::youtube::escape_markdown_v2(" از قبل توی دیتابیس ذخیره‌اند. چیزی به لیست اضافه نشد.");
    send_text_md(api, chat_id, &format!("{prefix}{rendered}{suffix}")).await
}

async fn filter_duplicates(
    client: &tokio_postgres::Client,
    owner: i64,
    incoming: &mut Vec<PendingEmoji>,
    pending: &[PendingEmoji],
) -> Vec<PendingEmoji> {
    let ids: Vec<String> = incoming.iter().map(|e| e.custom_emoji_id.clone()).collect();
    let db_dupes: HashSet<String> = emoji_store::existing_custom_emoji_ids(client, owner, &ids)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    let pending_ids: HashSet<&str> = pending.iter().map(|e| e.custom_emoji_id.as_str()).collect();
    let mut duplicates = Vec::new();
    let mut kept = Vec::with_capacity(incoming.len());
    let mut seen_in_batch: HashSet<String> = HashSet::new();
    let mut reported_dups: HashSet<String> = HashSet::new();
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
            if let Ok(idx) = rest.parse::<usize>() { plus.push(idx); continue; }
        }
        if let Some(rest) = token.strip_prefix('-') {
            if let Ok(idx) = rest.parse::<usize>() { minus.push(idx); continue; }
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

fn build_import_report(a: &emoji_import::ImportAnalysis) -> String {
    use crate::i18n::{t, tf};
    if a.db_empty {
        format!(
            "{}\n\n{}\n\n{}",
            tf("emoji.import.file_stats", &[("packs", &a.file_packs.to_string()), ("items", &a.file_items.to_string())]),
            t("emoji.import.db_empty"),
            t("emoji.import.hint_empty"),
        )
    } else {
        format!(
            "{}\n\n{}\n\n{}\n{}\n{}",
            tf("emoji.import.file_stats", &[("packs", &a.file_packs.to_string()), ("items", &a.file_items.to_string())]),
            tf("emoji.import.db_stats", &[("packs", &a.db_packs.to_string()), ("items", &a.db_items.to_string()), ("dupes", &a.duplicate_items.to_string())]),
            t("emoji.import.hint_replace"),
            t("emoji.import.hint_merge"),
            t("emoji.import.hint_smart"),
        )
    }
}

fn gregorian_to_jalali(gy: i32, gm: u32, gd: u32) -> (i32, u32, u32) {
    let g_days_in_month = [31i32, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let j_days_in_month = [31i32, 31, 31, 31, 31, 31, 30, 30, 30, 30, 30, 29];
    let gy = gy;
    let gm = gm as i32;
    let gd = gd as i32;
    let mut g_day_no = 365 * (gy - 1600)
        + (gy - 1600 + 3) / 4
        - (gy - 1600 + 99) / 100
        + (gy - 1600 + 399) / 400;
    for i in 0..(gm - 1) as usize {
        g_day_no += g_days_in_month[i];
    }
    if gm > 2 && ((gy % 4 == 0 && gy % 100 != 0) || gy % 400 == 0) {
        g_day_no += 1;
    }
    g_day_no += gd - 1;
    let mut j_day_no = g_day_no - 79;
    let j_np = j_day_no / 12053;
    j_day_no %= 12053;
    let mut jy = 979 + 33 * j_np + 4 * (j_day_no / 1461);
    j_day_no %= 1461;
    if j_day_no >= 366 {
        jy += (j_day_no - 1) / 365;
        j_day_no = (j_day_no - 1) % 365;
    }
    let mut i = 0usize;
    while i < 11 && j_day_no >= j_days_in_month[i] {
        j_day_no -= j_days_in_month[i];
        i += 1;
    }
    (jy, (i as i32 + 1) as u32, (j_day_no + 1) as u32)
}

fn extract_19digit_ids(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        if word.len() == 19 && word.chars().all(|c| c.is_ascii_digit()) {
            if seen.insert(word.to_string()) {
                out.push(word.to_string());
            }
        }
    }
    out
}

pub fn extract_addemoji_pack_name(text: &str) -> Option<String> {
    for part in text.split_whitespace() {
        let rest = part
            .strip_prefix("https://t.me/addemoji/")
            .or_else(|| part.strip_prefix("http://t.me/addemoji/"))
            .or_else(|| part.strip_prefix("t.me/addemoji/"));
        let Some(rest) = rest else { continue };
        let name = rest.split('/').next()
            .and_then(|s| s.split('?').next())
            .and_then(|s| s.split('#').next())
            .unwrap_or("")
            .to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

async fn fetch_pack_emojis(api: &Bot, pack_name: &str) -> Vec<PendingEmoji> {
    let set = match api.get_sticker_set(
        &GetStickerSetParams::builder().name(pack_name).build()
    ).await {
        Ok(r) => r.result,
        Err(e) => {
            eprintln!("get_sticker_set failed for {pack_name}: {e}");
            return Vec::new();
        }
    };
    set.stickers.into_iter()
        .filter_map(|s| {
            let id = s.custom_emoji_id?;
            let fallback = s.emoji.unwrap_or_else(|| "?".to_string());
            Some(PendingEmoji { custom_emoji_id: id, fallback })
        })
        .collect()
}

pub async fn handle_addemoji_link(
    api: &Bot,
    message: &Message,
    user_id: i64,
    pack_name: &str,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let chat_id = message.chat.id;
    let Some(db) = database else {
        let _ = send_text(api, chat_id, &t("emoji.db_required")).await;
        return;
    };
    let client = db.client();

    let mut new_emojis = fetch_pack_emojis(api, pack_name).await;
    if new_emojis.is_empty() {
        let _ = send_text(api, chat_id, &tf("emoji.pack_link_empty", &[("name", pack_name)])).await;
        return;
    }

    let existing = match flow_manager.get(user_id) {
        FlowState::AwaitingEmojis { collected } => collected,
        FlowState::AwaitingPackChoice { collected } => collected,
        _ => Vec::new(),
    };

    let incoming = new_emojis.len();
    let duplicates = filter_duplicates(client, user_id, &mut new_emojis, &existing).await;

    if incoming > 0 && new_emojis.is_empty() && existing.is_empty() {
        let _ = send_all_duplicate_message(api, chat_id, &duplicates).await;
        return;
    }

    let mut collected = existing;
    collected.extend(new_emojis);

    let total_pages = emoji_panel::pending_total_pages(collected.len());
    let text = emoji_panel::format_pending_emojis(&collected, &duplicates, 0);
    let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
    let _ = api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages)))
            .build(),
    ).await;
    flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
}

/// Calls getCustomEmojiStickers, groups by set_name, and formats a MarkdownV2 message
/// with premium inline emoji + disabled-preview links.
async fn build_pack_links_text(
    api: &Bot,
    collected: &[PendingEmoji],
    ids: &[String],
) -> String {
    use std::collections::HashMap;
    use crate::youtube::escape_markdown_v2;

    if ids.is_empty() {
        return escape_markdown_v2(&t("emoji.pack_links_none"));
    }

    let stickers = match api
        .get_custom_emoji_stickers(
            &GetCustomEmojiStickersParams::builder()
                .custom_emoji_ids(ids.to_vec())
                .build(),
        )
        .await
    {
        Ok(r) => r.result,
        Err(e) => {
            eprintln!("get_custom_emoji_stickers failed: {e}");
            return escape_markdown_v2(&t("emoji.pack_links_none"));
        }
    };

    // Map custom_emoji_id → set_name
    let mut id_to_set: HashMap<String, String> = HashMap::new();
    for sticker in &stickers {
        if let (Some(eid), Some(sn)) = (&sticker.custom_emoji_id, &sticker.set_name) {
            id_to_set.insert(eid.clone(), sn.clone());
        }
    }

    // Group PendingEmoji by set_name, preserving order of first appearance
    let mut set_order: Vec<String> = Vec::new();
    let mut set_to_entries: HashMap<String, Vec<&PendingEmoji>> = HashMap::new();
    for emoji in collected {
        let key = id_to_set
            .get(&emoji.custom_emoji_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        if !set_to_entries.contains_key(&key) {
            set_order.push(key.clone());
        }
        set_to_entries.entry(key).or_default().push(emoji);
    }

    let mut lines = Vec::new();
    for set_name in &set_order {
        let entries = &set_to_entries[set_name];
        // Render each as premium inline emoji (MarkdownV2 inline image syntax)
        let emoji_line: String = entries
            .iter()
            .map(|e| format!("![{}](tg://emoji?id={})", e.fallback, e.custom_emoji_id))
            .collect::<Vec<_>>()
            .join("");
        if set_name == "unknown" {
            lines.push(format!("{}{}", emoji_line, escape_markdown_v2(":\n(پک ناشناخته)")));
        } else {
            lines.push(format!(
                "{}{}\n{}",
                emoji_line,
                escape_markdown_v2(":"),
                escape_markdown_v2(&format!("https://t.me/addemoji/{}", set_name))
            ));
        }
    }

    if lines.is_empty() {
        escape_markdown_v2(&t("emoji.pack_links_none"))
    } else {
        lines.join("\n\n")
    }
}
