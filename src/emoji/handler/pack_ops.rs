use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::{InlineKeyboardMarkup, ReplyMarkup, ReplyKeyboardRemove},
};

use crate::i18n::{t, tf};
use crate::emoji::{PendingEmoji, panel as emoji_panel, store as emoji_store};

use super::helpers::{edit_panel, send_with_ents};

pub(super) async fn send_cancel_and_panel(api: &Bot, chat_id: i64, trace_id: u64) {
    eprintln!("[emoji trace={trace_id} event=send_cancel_panel] chat_id={chat_id}");
    let r1 = api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(t("emoji.canceled"))
            .reply_markup(ReplyMarkup::ReplyKeyboardRemove(ReplyKeyboardRemove::builder().remove_keyboard(true).build()))
            .build(),
    ).await;
    eprintln!("[emoji trace={trace_id} event=cancel_msg_sent] ok={}", r1.is_ok());
    let r2 = api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(emoji_panel::main_panel_text())
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard()))
            .build(),
    ).await;
    eprintln!("[emoji trace={trace_id} event=panel_sent] ok={}", r2.is_ok());
}

pub(super) async fn add_emojis_to_pack(
    api: &Bot, chat_id: i64, collected: &[PendingEmoji],
    pack_id: i32, user_id: i64, client: &tokio_postgres::Client, trace_id: u64,
) {
    eprintln!(
        "[emoji trace={trace_id} event=add_to_pack_start] chat_id={chat_id} \
         pack_id={pack_id} user_id={user_id} emoji_count={}",
        collected.len()
    );
    let pack_name = emoji_store::list_packs(client, user_id).await.ok()
        .and_then(|packs| packs.into_iter().find(|p| p.id == pack_id).map(|p| p.name))
        .unwrap_or_else(|| pack_id.to_string());
    eprintln!("[emoji trace={trace_id} event=add_to_pack_name] pack_name={pack_name:?}");
    let mut added = 0;
    for emoji in collected {
        let smart = match emoji_store::allocate_smart_name(client, user_id, &emoji.fallback).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[emoji trace={trace_id} event=allocate_smart_name_failed] fallback={:?} err={e:?}", emoji.fallback);
                continue;
            }
        };
        eprintln!(
            "[emoji trace={trace_id} event=add_item_attempt] id={} fallback={:?} smart_name={smart:?}",
            emoji.custom_emoji_id, emoji.fallback
        );
        match emoji_store::add_item(client, user_id, pack_id, &emoji.custom_emoji_id, &emoji.fallback, &smart).await {
            Ok(item) => {
                eprintln!("[emoji trace={trace_id} event=add_item_ok] db_id={} smart_name={:?}", item.id, item.smart_name);
                added += 1;
            }
            Err(e) => {
                eprintln!("[emoji trace={trace_id} event=add_item_failed] err={e:?}");
            }
        }
    }
    eprintln!("[emoji trace={trace_id} event=add_to_pack_done] added={added} total={}", collected.len());
    let summary_text = tf("emoji.added_summary", &[("count", &added.to_string()), ("pack", &pack_name)]);
    let r = api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(summary_text)
            .reply_markup(ReplyMarkup::ReplyKeyboardRemove(ReplyKeyboardRemove::builder().remove_keyboard(true).build()))
            .build(),
    ).await;
    eprintln!("[emoji trace={trace_id} event=summary_sent] ok={}", r.is_ok());
    let r = api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(emoji_panel::main_panel_text())
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard()))
            .build(),
    ).await;
    eprintln!("[emoji trace={trace_id} event=panel_sent] ok={}", r.is_ok());
}

pub(super) async fn show_packs_menu(
    api: &Bot, chat_id: i64, message_id: i32,
    user_id: i64, client: &tokio_postgres::Client, trace_id: u64,
) {
    eprintln!("[emoji trace={trace_id} event=show_packs_menu] chat_id={chat_id} user_id={user_id}");
    let packs = match emoji_store::list_packs(client, user_id).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[emoji trace={trace_id} event=list_packs_failed] err={e:?}");
            return;
        }
    };
    eprintln!("[emoji trace={trace_id} event=packs_loaded] count={}", packs.len());
    if packs.is_empty() {
        let keyboard = InlineKeyboardMarkup::builder()
            .inline_keyboard(vec![vec![emoji_panel::btn(&t("emoji.panel.back"), emoji_panel::CB_BACK)]])
            .build();
        let r = api.send_message(
            &SendMessageParams::builder().chat_id(chat_id).text(t("emoji.no_packs"))
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard)).build(),
        ).await;
        eprintln!("[emoji trace={trace_id} event=no_packs_sent] ok={}", r.is_ok());
        return;
    }
    edit_panel(api, chat_id, message_id, "📁 مجموعه‌ها:", Some(emoji_panel::packs_keyboard(&packs)), trace_id).await;
}

pub(super) async fn show_pack_detail(
    api: &Bot, chat_id: i64, message_id: i32,
    user_id: i64, pack_id: i32, client: &tokio_postgres::Client, trace_id: u64,
) {
    eprintln!("[emoji trace={trace_id} event=show_pack_detail] chat_id={chat_id} pack_id={pack_id}");
    let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
    let Some(pack) = packs.into_iter().find(|p| p.id == pack_id) else {
        eprintln!("[emoji trace={trace_id} event=pack_not_found] pack_id={pack_id}");
        return;
    };
    eprintln!("[emoji trace={trace_id} event=pack_found] name={:?} items={}", pack.name, pack.item_count);
    edit_panel(api, chat_id, message_id, &emoji_panel::pack_detail_text(&pack), Some(emoji_panel::pack_detail_keyboard(&pack)), trace_id).await;
}
