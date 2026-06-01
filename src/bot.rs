use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::{ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, MessageEntity, ReplyMarkup},
};

use crate::i18n::{entities_for_text, t};

pub const START_BUTTON_CALLBACK: &str = "say_hello";

pub async fn send_text(
    api: &Bot,
    chat_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (rendered, entities) = expand_and_entify(text).await;
    let params = if entities.is_empty() {
        SendMessageParams::builder().chat_id(chat_id).text(rendered).build()
    } else {
        SendMessageParams::builder().chat_id(chat_id).text(rendered).entities(entities).build()
    };
    api.send_message(&params).await?;
    Ok(())
}

/// Expands `{key}` templates via the emoji cache (if loaded), then collects
/// entities for both the cache expansions and the UI emoji in the remaining text.
async fn expand_and_entify(text: &str) -> (String, Vec<MessageEntity>) {
    if text.contains('{') {
        if let Some(cache_arc) = crate::emoji::cache::global() {
            let cache = cache_arc.read().await;
            if !cache.is_empty() {
                let (rendered, mut cache_ents) = cache.render_plain(text);
                let ui_ents = entities_for_text(&rendered);
                cache_ents.extend(ui_ents);
                cache_ents.sort_by_key(|e| e.offset);
                return (rendered, cache_ents);
            }
        }
    }
    let entities = entities_for_text(text);
    (text.to_string(), entities)
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
