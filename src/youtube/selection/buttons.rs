use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::AnswerCallbackQueryParams,
    types::{ButtonStyle, CallbackQuery, InlineKeyboardButton},
};

use crate::i18n::t;

use super::constants::CB_NOP;

pub fn button(text: &str, callback_data: String, style: Option<ButtonStyle>) -> InlineKeyboardButton {
    InlineKeyboardButton {
        text: text.to_string(),
        icon_custom_emoji_id: None,
        callback_data: Some(callback_data),
        style,
        url: None,
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

pub fn header_button(text: &str) -> InlineKeyboardButton {
    button(text, CB_NOP.to_string(), None)
}

pub fn plain_button(text: &str, callback_data: String) -> InlineKeyboardButton {
    button(text, callback_data, None)
}

pub fn choice_button(text: &str, callback_data: String, selected: bool) -> InlineKeyboardButton {
    let style = if selected { Some(ButtonStyle::Success) } else { None };
    button(text, callback_data, style)
}

pub fn confirm_button(text: &str, callback_data: String) -> InlineKeyboardButton {
    button(text, callback_data, Some(ButtonStyle::Success))
}

pub fn primary_button(text: &str, callback_data: String) -> InlineKeyboardButton {
    button(text, callback_data, Some(ButtonStyle::Primary))
}

pub fn main_menu_button() -> InlineKeyboardButton {
    let icon_id = t("emoji.panel.icons.back");
    InlineKeyboardButton {
        text: t("start.back"),
        icon_custom_emoji_id: if icon_id.is_empty() || icon_id.starts_with('!') { None } else { Some(icon_id) },
        callback_data: Some(crate::bot::CB_START_PANEL.to_string()),
        style: Some(ButtonStyle::Primary),
        url: None, login_url: None, web_app: None,
        switch_inline_query: None, switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None, copy_text: None,
        callback_game: None, pay: None,
    }
}

pub async fn answer(api: &Bot, cq: &CallbackQuery, text_key: &str) {
    let mut params = AnswerCallbackQueryParams::builder()
        .callback_query_id(&cq.id)
        .build();
    if !text_key.is_empty() {
        params.text = Some(t(text_key));
    }
    let _ = api.answer_callback_query(&params).await;
}

pub fn quality_label(height: u32) -> String {
    let key = format!("youtube.quality.buttons.{height}");
    let label = t(&key);
    if label.starts_with('!') { format!("{height}p") } else { label }
}
