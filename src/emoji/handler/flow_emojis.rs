use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{GetCustomEmojiStickersParams, SendMessageParams},
    types::{Message, ReplyMarkup},
};

use crate::i18n::{t, tf};
use crate::emoji::{FlowManager, FlowState, PendingEmoji, panel as emoji_panel, store as emoji_store};

use super::{
    addemoji::extract_19digit_ids,
    extract::extract_custom_emojis,
    helpers::{filter_duplicates, send_all_duplicate_message, send_with_ents},
    pack_ops::send_cancel_and_panel,
};

pub(super) async fn handle(
    api: &Bot, message: &Message, chat_id: i64, user_id: i64,
    flow_manager: &mut FlowManager, client: &tokio_postgres::Client,
    mut collected: Vec<PendingEmoji>,
) -> bool {
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
            &GetCustomEmojiStickersParams::builder().custom_emoji_ids(id_hits).build(),
        ).await.map(|r| r.result).unwrap_or_default();
        let mut from_ids: Vec<PendingEmoji> = stickers.into_iter()
            .filter_map(|s| Some(PendingEmoji { custom_emoji_id: s.custom_emoji_id?, fallback: s.emoji.unwrap_or_else(|| "?".to_string()) }))
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
            &SendMessageParams::builder().chat_id(chat_id).text(text).parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages))).build(),
        ).await;
        flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
        return true;
    }

    let mut new_emojis = extract_custom_emojis(message);
    if new_emojis.is_empty() && collected.is_empty() {
        let _ = crate::bot::send_text(api, chat_id, &t("emoji.no_emoji_found")).await;
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
        &SendMessageParams::builder().chat_id(chat_id).text(text).parse_mode(ParseMode::MarkdownV2)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages))).build(),
    ).await;
    flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
    true
}
