use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::{InlineKeyboardMarkup, Message, ReplyMarkup, ReplyKeyboardRemove},
};

use crate::i18n::{t, tf};
use crate::emoji::{PendingEmoji, panel as emoji_panel, store as emoji_store};

use super::helpers::{edit_panel, send_with_ents};

pub(super) async fn send_cancel_and_panel(api: &Bot, chat_id: i64) {
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

pub(super) async fn add_emojis_to_pack(
    api: &Bot, chat_id: i64, collected: &[PendingEmoji],
    pack_id: i32, user_id: i64, client: &tokio_postgres::Client,
) {
    let pack_name = emoji_store::list_packs(client, user_id).await.ok()
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

pub(super) async fn show_packs_menu(api: &Bot, chat_id: i64, message_id: i32, user_id: i64, client: &tokio_postgres::Client) {
    let packs = match emoji_store::list_packs(client, user_id).await {
        Ok(p) => p,
        Err(e) => { eprintln!("list_packs failed: {e}"); return; }
    };
    if packs.is_empty() {
        let keyboard = InlineKeyboardMarkup::builder()
            .inline_keyboard(vec![vec![emoji_panel::btn(&t("emoji.panel.back"), emoji_panel::CB_BACK)]])
            .build();
        let _ = api.send_message(
            &SendMessageParams::builder().chat_id(chat_id).text(t("emoji.no_packs"))
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard)).build(),
        ).await;
        return;
    }
    edit_panel(api, chat_id, message_id, "📁 مجموعه‌ها:", Some(emoji_panel::packs_keyboard(&packs))).await;
}

pub(super) async fn show_pack_detail(api: &Bot, chat_id: i64, message_id: i32, user_id: i64, pack_id: i32, client: &tokio_postgres::Client) {
    let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
    let Some(pack) = packs.into_iter().find(|p| p.id == pack_id) else { return };
    edit_panel(api, chat_id, message_id, &emoji_panel::pack_detail_text(&pack), Some(emoji_panel::pack_detail_keyboard(&pack))).await;
}
