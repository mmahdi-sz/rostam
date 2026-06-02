use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{AnswerCallbackQueryParams, SendMessageParams},
    types::{ButtonStyle, CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup},
};

use crate::i18n::{entities_for_text, t};

use super::trace::log_trace;
use super::types::VideoInfo;

const CB_QUALITY_PREFIX: &str = "yt:quality:";
const QUALITY_ICON_KEY: &str = "export";

const QUALITY_OPTIONS: &[(u32, &str)] = &[
    (4320, "youtube.quality.buttons.4320"),
    (2160, "youtube.quality.buttons.2160"),
    (1440, "youtube.quality.buttons.1440"),
    (1080, "youtube.quality.buttons.1080"),
    (720, "youtube.quality.buttons.720"),
    (480, "youtube.quality.buttons.480"),
    (360, "youtube.quality.buttons.360"),
    (240, "youtube.quality.buttons.240"),
    (144, "youtube.quality.buttons.144"),
];

pub async fn send_quality_prompt(
    trace_id: u64,
    api: &Bot,
    chat_id: i64,
    info: &VideoInfo,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = quality_options(info);
    if options.is_empty() {
        log_trace(trace_id, "quality_prompt_skipped", &format!("available_heights={:?}", info.available_heights));
        return Ok(());
    }

    let button_heights: Vec<u32> = options.iter().map(|(height, _)| *height).collect();
    log_trace(trace_id, "quality_prompt_buttons", &format!("available_heights={:?} button_heights={button_heights:?}", info.available_heights));
    let text = t("youtube.quality.prompt");
    let entities = entities_for_text(&text);
    let mut params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(quality_keyboard(&options)))
        .build();

    if !entities.is_empty() {
        params.entities = Some(entities);
    }

    api.send_message(&params).await?;
    log_trace(trace_id, "quality_prompt_sent", &format!("chat_id={chat_id}"));
    Ok(())
}

pub async fn handle_quality_callback(api: &Bot, callback_query: &CallbackQuery) -> bool {
    let Some(data) = callback_query.data.as_deref() else {
        return false;
    };
    if !data.starts_with(CB_QUALITY_PREFIX) {
        return false;
    }
    eprintln!("[youtube callback event=quality_clicked] user_id={} data={data}", callback_query.from.id);

    let params = AnswerCallbackQueryParams::builder()
        .callback_query_id(&callback_query.id)
        .text(t("youtube.quality.not_ready"))
        .build();
    let _ = api.answer_callback_query(&params).await;
    true
}

fn quality_keyboard(options: &[(u32, &str)]) -> InlineKeyboardMarkup {
    let rows = options
        .iter()
        .map(|(height, label_key)| {
            vec![quality_button(
                &t(label_key),
                &format!("{CB_QUALITY_PREFIX}{height}"),
                button_style(*height),
            )]
        })
        .collect();

    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

fn quality_options(info: &VideoInfo) -> Vec<(u32, &'static str)> {
    let exact: Vec<(u32, &str)> = QUALITY_OPTIONS
        .iter()
        .copied()
        .filter(|(height, _)| info.available_heights.contains(height))
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let Some(max_height) = info.available_heights.iter().copied().max() else {
        return Vec::new();
    };
    QUALITY_OPTIONS
        .iter()
        .copied()
        .filter(|(height, _)| *height <= max_height)
        .collect()
}

fn button_style(height: u32) -> ButtonStyle {
    if height >= 1080 {
        ButtonStyle::Success
    } else if height <= 360 {
        ButtonStyle::Danger
    } else {
        ButtonStyle::Primary
    }
}

fn quality_button(text: &str, callback_data: &str, style: ButtonStyle) -> InlineKeyboardButton {
    let icon_id = t(&format!("emoji.panel.icons.{QUALITY_ICON_KEY}"));
    InlineKeyboardButton {
        text: text.to_string(),
        icon_custom_emoji_id: if icon_id.is_empty() || icon_id.starts_with('!') {
            None
        } else {
            Some(icon_id)
        },
        callback_data: Some(callback_data.to_string()),
        style: Some(style),
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
