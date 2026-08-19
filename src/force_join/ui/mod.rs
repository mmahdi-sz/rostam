pub mod admin;
pub mod gate;

pub use admin::*;
pub use gate::*;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardButton, InlineKeyboardMarkup, LinkPreviewOptions},
};

use crate::i18n::t;

// Phase 4 Coverage Note: `no_preview`, `send_text_np`, and `edit_text_np` are thin wrappers
// around Frankenstein API calls setting `is_disabled(true)`. They are intentionally not directly
// unit-tested as their formatting and transmission behavior is verified indirectly via the
// Phase 3 TestAPI endpoint suite and dispatch integration flows.
pub(crate) fn no_preview() -> LinkPreviewOptions {
    LinkPreviewOptions::builder().is_disabled(true).build()
}

/// Like `crate::bot::edit_text` but always disables link preview —
/// locks display raw link text and should not generate previews.
pub(crate) async fn edit_text_np(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    text: &str,
    kb: Option<InlineKeyboardMarkup>,
) {
    let _ = match kb {
        Some(kb) => {
            api.edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(message_id)
                    .text(text)
                    .link_preview_options(no_preview())
                    .reply_markup(kb)
                    .build(),
            )
            .await
        }
        None => {
            api.edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(message_id)
                    .text(text)
                    .link_preview_options(no_preview())
                    .build(),
            )
            .await
        }
    };
}

/// Like `crate::bot::send_text` but always disables link preview.
pub(crate) async fn send_text_np(api: &Bot, chat_id: i64, text: &str) {
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .link_preview_options(no_preview())
        .build();
    let _ = api.send_message(&params).await;
}

pub(crate) fn url_button(text: &str, url: &str) -> InlineKeyboardButton {
    let icon_id = t("emoji.panel.icons.telegram_logo");
    InlineKeyboardButton {
        text: text.to_string(),
        icon_custom_emoji_id: if icon_id.is_empty() || icon_id.starts_with('!') {
            None
        } else {
            Some(icon_id)
        },
        callback_data: None,
        style: None,
        url: Some(url.to_string()),
        login_url: None,
        web_app: None,
        switch_inline_query: None,
        switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None,
        copy_text: None,
        callback_game: None,
        pay: None,
    }
}
