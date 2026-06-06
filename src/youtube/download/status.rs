use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::EditMessageTextParams,
    types::{ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup},
};

use crate::i18n::{entities_for_text, t};

pub const CB_CANCEL_PREFIX: &str = "yt:cancel:";

pub fn cancel_keyboard(request_id: u64) -> InlineKeyboardMarkup {
    let button = InlineKeyboardButton {
        text: t("youtube.download.cancel_button"),
        callback_data: Some(format!("{CB_CANCEL_PREFIX}{request_id}")),
        style: Some(ButtonStyle::Danger),
        icon_custom_emoji_id: None,
        url: None,
        login_url: None,
        web_app: None,
        switch_inline_query: None,
        switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None,
        copy_text: None,
        callback_game: None,
        pay: None,
    };
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![button]])
        .build()
}

pub async fn edit_progress_status(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    text: String,
    request_id: u64,
) {
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .reply_markup(cancel_keyboard(request_id))
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    if let Err(error) = api.edit_message_text(&params).await {
        let desc = error.to_string();
        if !desc.contains("message is not modified") {
            eprintln!("[youtube event=edit_progress_status_failed] {desc}");
        }
    }
}

pub async fn edit_status(api: &Bot, chat_id: i64, message_id: i32, text: String) {
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    if let Err(error) = api.edit_message_text(&params).await {
        let desc = error.to_string();
        if !desc.contains("message is not modified") {
            eprintln!("[youtube event=edit_status_failed] {desc}");
        }
    }
}
