use std::collections::HashSet;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, LinkPreviewOptions, ReplyMarkup},
};

use crate::bot::send_text_md;
use crate::i18n::entities_for_text;
use crate::youtube::escape_markdown_v2;
use crate::emoji::{PendingEmoji, store as emoji_store};

pub(super) async fn send_with_ents(api: &Bot, chat_id: i64, text: String, reply_markup: Option<ReplyMarkup>) {
    let ents = entities_for_text(&text);
    let params = match (ents.is_empty(), reply_markup) {
        (true, None) => SendMessageParams::builder().chat_id(chat_id).text(text).build(),
        (true, Some(rm)) => SendMessageParams::builder().chat_id(chat_id).text(text).reply_markup(rm).build(),
        (false, None) => SendMessageParams::builder().chat_id(chat_id).text(text).entities(ents).build(),
        (false, Some(rm)) => SendMessageParams::builder().chat_id(chat_id).text(text).entities(ents).reply_markup(rm).build(),
    };
    let _ = api.send_message(&params).await;
}

pub(super) async fn edit_panel(api: &Bot, chat_id: i64, message_id: i32, text: &str, keyboard: Option<InlineKeyboardMarkup>) {
    let ents = entities_for_text(text);
    let np = || LinkPreviewOptions::builder().is_disabled(true).build();
    let params = match (ents.is_empty(), keyboard) {
        (true, None) => EditMessageTextParams::builder().chat_id(chat_id).message_id(message_id).text(text).link_preview_options(np()).build(),
        (true, Some(kb)) => EditMessageTextParams::builder().chat_id(chat_id).message_id(message_id).text(text).link_preview_options(np()).reply_markup(kb).build(),
        (false, None) => EditMessageTextParams::builder().chat_id(chat_id).message_id(message_id).text(text).entities(ents).link_preview_options(np()).build(),
        (false, Some(kb)) => EditMessageTextParams::builder().chat_id(chat_id).message_id(message_id).text(text).entities(ents).link_preview_options(np()).reply_markup(kb).build(),
    };
    if let Err(e) = api.edit_message_text(&params).await {
        eprintln!("edit_message_text failed: {e}");
    }
}

pub(super) async fn send_all_duplicate_message(
    api: &Bot, chat_id: i64, duplicates: &[PendingEmoji],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut rendered = String::new();
    for d in duplicates {
        rendered.push_str(&format!("![{}](tg://emoji?id={})", d.fallback, d.custom_emoji_id));
    }
    let prefix = escape_markdown_v2("⚠️ همه‌ی ایموجی‌های ");
    let suffix = escape_markdown_v2(" از قبل توی دیتابیس ذخیره‌اند. چیزی به لیست اضافه نشد.");
    send_text_md(api, chat_id, &format!("{prefix}{rendered}{suffix}")).await
}

pub(super) async fn filter_duplicates(
    client: &tokio_postgres::Client,
    owner: i64,
    incoming: &mut Vec<PendingEmoji>,
    pending: &[PendingEmoji],
) -> Vec<PendingEmoji> {
    let ids: Vec<String> = incoming.iter().map(|e| e.custom_emoji_id.clone()).collect();
    let db_dupes: HashSet<String> = emoji_store::existing_custom_emoji_ids(client, owner, &ids)
        .await.unwrap_or_default().into_iter().collect();
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
            if reported_dups.insert(emoji.custom_emoji_id.clone()) { duplicates.push(emoji); }
        } else {
            seen_in_batch.insert(emoji.custom_emoji_id.clone());
            kept.push(emoji);
        }
    }
    *incoming = kept;
    duplicates
}
