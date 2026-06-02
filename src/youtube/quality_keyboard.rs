use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{AnswerCallbackQueryParams, SendMessageParams},
    types::{ButtonStyle, CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup},
};

use crate::i18n::{entities_for_text, t};

const CB_QUALITY_PREFIX: &str = "yt:quality:";
const QUALITY_ICON_KEY: &str = "export";

const QUALITY_OPTIONS: &[(&str, &str)] = &[
    ("4320", "youtube.quality.buttons.4320"),
    ("2160", "youtube.quality.buttons.2160"),
    ("1440", "youtube.quality.buttons.1440"),
    ("1080", "youtube.quality.buttons.1080"),
    ("720", "youtube.quality.buttons.720"),
    ("480", "youtube.quality.buttons.480"),
    ("360", "youtube.quality.buttons.360"),
    ("240", "youtube.quality.buttons.240"),
    ("144", "youtube.quality.buttons.144"),
];

pub async fn send_quality_prompt(
    api: &Bot,
    chat_id: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let text = t("youtube.quality.prompt");
    let entities = entities_for_text(&text);
    let mut params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(quality_keyboard()))
        .build();

    if !entities.is_empty() {
        params.entities = Some(entities);
    }

    api.send_message(&params).await?;
    Ok(())
}

pub async fn handle_quality_callback(api: &Bot, callback_query: &CallbackQuery) -> bool {
    let Some(data) = callback_query.data.as_deref() else {
        return false;
    };
    if !data.starts_with(CB_QUALITY_PREFIX) {
        return false;
    }

    let params = AnswerCallbackQueryParams::builder()
        .callback_query_id(&callback_query.id)
        .text(t("youtube.quality.not_ready"))
        .build();
    let _ = api.answer_callback_query(&params).await;
    true
}

fn quality_keyboard() -> InlineKeyboardMarkup {
    let rows = QUALITY_OPTIONS
        .iter()
        .map(|(height, label_key)| {
            vec![quality_button(
                &t(label_key),
                &format!("{CB_QUALITY_PREFIX}{height}"),
            )]
        })
        .collect();

    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

fn quality_button(text: &str, callback_data: &str) -> InlineKeyboardButton {
    let icon_id = t(&format!("emoji.panel.icons.{QUALITY_ICON_KEY}"));
    InlineKeyboardButton {
        text: text.to_string(),
        icon_custom_emoji_id: if icon_id.is_empty() || icon_id.starts_with('!') {
            None
        } else {
            Some(icon_id)
        },
        callback_data: Some(callback_data.to_string()),
        style: Some(ButtonStyle::Primary),
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
