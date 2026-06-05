use frankenstein::types::{ButtonStyle, InlineKeyboardButton};

use crate::i18n::t;

pub fn btn(text: &str, callback_data: &str) -> InlineKeyboardButton {
    btn_icon(text, callback_data, "")
}

pub fn btn_success(text: &str, callback_data: &str) -> InlineKeyboardButton {
    btn_icon_success(text, callback_data, "")
}

pub fn btn_danger(text: &str, callback_data: &str) -> InlineKeyboardButton {
    btn_icon_danger(text, callback_data, "")
}

pub fn btn_icon_success(text: &str, callback_data: &str, icon_key: &str) -> InlineKeyboardButton {
    let icon_id = resolve_icon(icon_key);
    InlineKeyboardButton {
        text: text.to_string(),
        icon_custom_emoji_id: icon_id,
        callback_data: Some(callback_data.to_string()),
        style: Some(ButtonStyle::Success),
        url: None, login_url: None, web_app: None,
        switch_inline_query: None, switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None, copy_text: None,
        callback_game: None, pay: None,
    }
}

pub fn btn_icon_danger(text: &str, callback_data: &str, icon_key: &str) -> InlineKeyboardButton {
    let icon_id = resolve_icon(icon_key);
    InlineKeyboardButton {
        text: text.to_string(),
        icon_custom_emoji_id: icon_id,
        callback_data: Some(callback_data.to_string()),
        style: Some(ButtonStyle::Danger),
        url: None, login_url: None, web_app: None,
        switch_inline_query: None, switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None, copy_text: None,
        callback_game: None, pay: None,
    }
}

fn resolve_icon(icon_key: &str) -> Option<String> {
    if icon_key.is_empty() { return None; }
    let id = t(&format!("emoji.panel.icons.{icon_key}"));
    if id.is_empty() || id.starts_with('!') { None } else { Some(id) }
}

pub fn btn_icon(text: &str, callback_data: &str, icon_key: &str) -> InlineKeyboardButton {
    let icon_id = if icon_key.is_empty() {
        None
    } else {
        let id = t(&format!("emoji.panel.icons.{icon_key}"));
        if id.is_empty() || id.starts_with('!') { None } else { Some(id) }
    };
    InlineKeyboardButton {
        text: text.to_string(),
        icon_custom_emoji_id: icon_id,
        callback_data: Some(callback_data.to_string()),
        style: Some(ButtonStyle::Primary),
        url: None, login_url: None, web_app: None,
        switch_inline_query: None, switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None, copy_text: None,
        callback_game: None, pay: None,
    }
}

pub fn btn_icon_plain(text: &str, callback_data: &str, icon_key: &str) -> InlineKeyboardButton {
    let icon_id = if icon_key.is_empty() {
        None
    } else {
        let id = t(&format!("emoji.panel.icons.{icon_key}"));
        if id.is_empty() || id.starts_with('!') { None } else { Some(id) }
    };
    InlineKeyboardButton {
        text: text.to_string(),
        icon_custom_emoji_id: icon_id,
        callback_data: Some(callback_data.to_string()),
        style: None,
        url: None, login_url: None, web_app: None,
        switch_inline_query: None, switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None, copy_text: None,
        callback_game: None, pay: None,
    }
}
