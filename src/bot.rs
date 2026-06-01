use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::{ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup},
};

use crate::i18n::{entities_for_text, t};

pub const START_BUTTON_CALLBACK: &str = "say_hello";

pub async fn send_text(
    api: &Bot,
    chat_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let entities = entities_for_text(text);
    let params = if entities.is_empty() {
        SendMessageParams::builder().chat_id(chat_id).text(text).build()
    } else {
        SendMessageParams::builder().chat_id(chat_id).text(text).entities(entities).build()
    };
    api.send_message(&params).await?;
    Ok(())
}

pub async fn send_text_md(
    api: &Bot,
    chat_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .parse_mode(ParseMode::MarkdownV2)
            .build(),
    )
    .await?;
    Ok(())
}

pub async fn send_start_button(
    api: &Bot,
    chat_id: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let button = InlineKeyboardButton::builder()
        .text(t("start.button"))
        .callback_data(START_BUTTON_CALLBACK)
        .style(ButtonStyle::Success)
        .build();

    let keyboard = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![button]])
        .build();

    api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(t("start.prompt"))
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard))
            .build(),
    )
    .await?;
    Ok(())
}
