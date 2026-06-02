use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::{Message, ReplyKeyboardRemove, ReplyMarkup},
};

use crate::i18n::t;
use crate::emoji::{FlowManager, FlowState, PendingEmoji, panel as emoji_panel, store as emoji_store};

use super::{
    extract::extract_custom_emojis,
    helpers::{filter_duplicates, send_all_duplicate_message, send_with_ents},
    pack_ops::send_cancel_and_panel,
    pending::apply_edit_ops,
};

pub(super) async fn handle(
    api: &Bot, message: &Message, chat_id: i64, user_id: i64,
    flow_manager: &mut FlowManager, client: &tokio_postgres::Client,
    mut collected: Vec<PendingEmoji>,
) -> bool {
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
            &SendMessageParams::builder().chat_id(chat_id).text(summary).parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages))).build(),
        ).await;
        flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
        return true;
    }

    if text.starts_with('-') || text.starts_with('+') {
        if apply_edit_ops(&mut collected, text).is_err() {
            let _ = crate::bot::send_text(api, chat_id, &t("emoji.pending.mixed_ops")).await;
            flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
            return true;
        }
        let total_pages = emoji_panel::pending_total_pages(collected.len());
        let summary = emoji_panel::format_pending_emojis(&collected, &[], 0);
        let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
        let _ = api.send_message(
            &SendMessageParams::builder().chat_id(chat_id).text(summary).parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages))).build(),
        ).await;
        flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
        return true;
    }

    if text.is_empty() { return true; }

    let pack = match emoji_store::find_pack_by_name(client, user_id, text).await {
        Ok(Some(p)) => p,
        Ok(None) => match emoji_store::create_pack(client, user_id, text).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("create_pack failed: {e:?}");
                let _ = crate::bot::send_text(api, chat_id, &t("emoji.pack_create_failed")).await;
                flow_manager.clear(user_id);
                return true;
            }
        },
        Err(e) => {
            eprintln!("find_pack_by_name failed: {e:?}");
            let _ = crate::bot::send_text(api, chat_id, &t("emoji.pack_create_failed")).await;
            flow_manager.clear(user_id);
            return true;
        }
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
        crate::i18n::tf("emoji.added_summary", &[("count", &added.to_string()), ("pack", &pack.name)]),
        Some(ReplyMarkup::ReplyKeyboardRemove(ReplyKeyboardRemove::builder().remove_keyboard(true).build()))).await;
    send_with_ents(api, chat_id, emoji_panel::main_panel_text(),
        Some(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard()))).await;
    flow_manager.clear(user_id);
    true
}
