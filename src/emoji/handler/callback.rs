use std::fs;

use chrono::{Datelike, Timelike, TimeZone};
use chrono_tz::Asia::Tehran;
use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{
        AnswerCallbackQueryParams, EditMessageTextParams, SendDocumentParams, SendMessageParams,
    },
    types::{
        InlineKeyboardMarkup, LinkPreviewOptions, MaybeInaccessibleMessage, MessageEntity,
        ReplyMarkup, ReplyKeyboardRemove,
    },
};

use crate::bot::send_text;
use crate::database::postgresql::PostgresDatabase;
use crate::i18n::{entities_for_text, t, tf};
use crate::youtube::escape_markdown_v2;
use crate::youtube::jalali::gregorian_to_jalali;
use crate::emoji::{
    FlowManager, FlowState,
    panel::{self as emoji_panel, *},
    store as emoji_store, import as emoji_import,
};

use super::{
    helpers::{edit_panel, send_with_ents},
    list::{send_emoji_list, edit_emoji_list_page},
    pack_ops::{show_packs_menu, show_pack_detail, add_emojis_to_pack},
    pack_links::build_pack_links_text,
};

pub async fn handle_emoji_callback(
    api: &Bot, cbq: &frankenstein::types::CallbackQuery,
    flow_manager: &mut FlowManager, database: &Option<PostgresDatabase>,
) {
    let _ = api.answer_callback_query(
        &AnswerCallbackQueryParams::builder().callback_query_id(&cbq.id).build(),
    ).await;

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
                    let (jy, jm, jd) = gregorian_to_jalali(now.year(), now.month() as i32, now.day() as i32);
                    let filename = format!("emoji_{:04}-{:02}-{:02}_{:02}-{:02}.sql", jy, jm, jd, now.hour(), now.minute());
                    let path = std::env::temp_dir().join(&filename);
                    if let Err(e) = fs::write(&path, &sql) {
                        eprintln!("write export file failed: {e}");
                        let _ = send_text(api, chat_id, &t("emoji.export_failed")).await;
                    } else {
                        let _ = api.send_document(
                            &SendDocumentParams::builder()
                                .chat_id(chat_id).document(FileUpload::InputFile(InputFile { path: path.clone() }))
                                .caption(t("emoji.export_caption")).build(),
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
        d if d.starts_with(CB_LIST_PAGE_PREFIX) => {
            if let Some(page) = d.strip_prefix(CB_LIST_PAGE_PREFIX).and_then(|s| s.parse::<usize>().ok()) {
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
                    .await.ok()
                    .and_then(|packs| packs.into_iter().find(|p| p.id == pack_id))
                    .map(|p| p.name).unwrap_or_default();
                if let Err(e) = emoji_store::delete_pack(client, user_id, pack_id).await {
                    eprintln!("delete_pack failed: {e}");
                }
                let msg = tf("emoji.pack_deleted", &[("name", &name)]);
                let ents = entities_for_text(&msg);
                let no_preview = LinkPreviewOptions::builder().is_disabled(true).build();
                let params = if ents.is_empty() {
                    SendMessageParams::builder().chat_id(chat_id).text(&msg).link_preview_options(no_preview).build()
                } else {
                    SendMessageParams::builder().chat_id(chat_id).text(&msg).entities(ents).link_preview_options(no_preview).build()
                };
                let _ = api.send_message(&params).await;
                send_with_ents(api, chat_id, emoji_panel::main_panel_text(),
                    Some(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard()))).await;
            }
        }
        d if d == CB_SHOW_PACK_LINKS => {
            if let FlowState::AwaitingPackChoice { collected } = flow_manager.get(user_id) {
                let ids: Vec<String> = collected.iter().map(|e| e.custom_emoji_id.clone()).collect();
                let text = build_pack_links_text(api, &collected, &ids).await;
                let no_preview = LinkPreviewOptions::builder().is_disabled(true).build();
                let params = EditMessageTextParams::builder()
                    .chat_id(chat_id).message_id(message_id).text(&text)
                    .parse_mode(ParseMode::MarkdownV2).link_preview_options(no_preview)
                    .reply_markup(emoji_panel::pack_links_keyboard()).build();
                if let Err(e) = api.edit_message_text(&params).await { eprintln!("edit pack links failed: {e}"); }
            }
        }
        d if d == CB_BACK_TO_PACK_CHOICE => {
            if let FlowState::AwaitingPackChoice { collected } = flow_manager.get(user_id) {
                let total_pages = emoji_panel::pending_total_pages(collected.len());
                let summary = emoji_panel::format_pending_emojis(&collected, &[], 0);
                let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
                let params = EditMessageTextParams::builder()
                    .chat_id(chat_id).message_id(message_id).text(&summary)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages)).build();
                if let Err(e) = api.edit_message_text(&params).await { eprintln!("edit back to pack choice failed: {e}"); }
            }
        }
        d if d.starts_with(CB_PENDING_PAGE_PREFIX) => {
            if let Some(page) = d.strip_prefix(CB_PENDING_PAGE_PREFIX).and_then(|s| s.parse::<usize>().ok()) {
                if let FlowState::AwaitingPackChoice { collected } = flow_manager.get(user_id) {
                    let total_pages = emoji_panel::pending_total_pages(collected.len());
                    let text = emoji_panel::format_pending_emojis(&collected, &[], page);
                    let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
                    let params = EditMessageTextParams::builder()
                        .chat_id(chat_id).message_id(message_id).text(&text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(emoji_panel::pack_choice_keyboard(&packs, page, total_pages)).build();
                    if let Err(e) = api.edit_message_text(&params).await { eprintln!("edit pending page failed: {e}"); }
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
