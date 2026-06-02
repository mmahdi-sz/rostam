use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, LinkPreviewOptions, ReplyMarkup},
};

use crate::i18n::t;
use crate::emoji::{panel as emoji_panel, store as emoji_store};

pub(super) async fn send_emoji_list(api: &Bot, chat_id: i64, user_id: i64, client: &tokio_postgres::Client) {
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
            .chat_id(chat_id).text(text).parse_mode(ParseMode::MarkdownV2)
            .link_preview_options(no_preview)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard)).build(),
    ).await;
}

pub(super) async fn edit_emoji_list_page(api: &Bot, chat_id: i64, message_id: i32, user_id: i64, client: &tokio_postgres::Client, page: usize) {
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
        .chat_id(chat_id).message_id(message_id).text(text)
        .parse_mode(ParseMode::MarkdownV2).link_preview_options(no_preview)
        .reply_markup(keyboard).build();
    if let Err(e) = api.edit_message_text(&params).await {
        eprintln!("edit_message_text failed: {e}");
    }
}
