use std::collections::HashSet;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{GetStickerSetParams, SendMessageParams},
    types::{Message, ReplyMarkup},
};

use crate::bot::send_text;
use crate::database::postgresql::PostgresDatabase;
use crate::i18n::tf;
use crate::emoji::{FlowManager, FlowState, PendingEmoji, panel as emoji_panel, store as emoji_store};

use super::helpers::{filter_duplicates, send_all_duplicate_message};

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
            .unwrap_or("").to_string();
        if !name.is_empty() { return Some(name); }
    }
    None
}

pub(super) fn extract_19digit_ids(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        if word.len() == 19 && word.chars().all(|c| c.is_ascii_digit()) {
            if seen.insert(word.to_string()) { out.push(word.to_string()); }
        }
    }
    out
}

pub(super) async fn fetch_pack_emojis(api: &Bot, pack_name: &str) -> Vec<PendingEmoji> {
    let set = match api.get_sticker_set(&GetStickerSetParams::builder().name(pack_name).build()).await {
        Ok(r) => r.result,
        Err(e) => { eprintln!("get_sticker_set failed for {pack_name}: {e}"); return Vec::new(); }
    };
    set.stickers.into_iter().filter_map(|s| {
        let id = s.custom_emoji_id?;
        let fallback = s.emoji.unwrap_or_else(|| "?".to_string());
        Some(PendingEmoji { custom_emoji_id: id, fallback })
    }).collect()
}

pub async fn handle_addemoji_link(
    api: &Bot, message: &Message, user_id: i64, pack_name: &str,
    flow_manager: &mut FlowManager, database: &Option<PostgresDatabase>,
) {
    let chat_id = message.chat.id;
    let Some(db) = database else {
        let _ = send_text(api, chat_id, &crate::i18n::t("emoji.db_required")).await;
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
            .chat_id(chat_id).text(text).parse_mode(ParseMode::MarkdownV2)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages)))
            .build(),
    ).await;
    flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
}
