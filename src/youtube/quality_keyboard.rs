use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{AnswerCallbackQueryParams, EditMessageTextParams, SendMessageParams},
    types::{
        ButtonStyle, CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup,
        MaybeInaccessibleMessage, ReplyMarkup,
    },
};

use crate::i18n::{entities_for_text, t, tf};

use super::trace::log_trace;
use super::types::{VideoCodec, VideoFormatOption, VideoInfo};

const CB_QUALITY_PREFIX: &str = "yt:quality:";
const CB_CODEC_PREFIX: &str = "yt:codec:";
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

const CODEC_ORDER: &[VideoCodec] = &[
    VideoCodec::H264,
    VideoCodec::H265,
    VideoCodec::Vp9,
    VideoCodec::Av1,
];

pub async fn send_quality_prompt(
    trace_id: u64,
    api: &Bot,
    chat_id: i64,
    info: &VideoInfo,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = quality_options(info);
    if options.is_empty() {
        log_trace(
            trace_id,
            "quality_prompt_skipped",
            &format!(
                "available_heights={:?} video_formats={}",
                info.available_heights,
                format_summary(&info.video_formats)
            ),
        );
        return Ok(());
    }

    let button_summary = options
        .iter()
        .map(|option| format!("{}:{:?}", option.height, option.codecs))
        .collect::<Vec<_>>()
        .join(",");
    log_trace(
        trace_id,
        "quality_prompt_buttons",
        &format!(
            "available_heights={:?} buttons={button_summary}",
            info.available_heights
        ),
    );
    let text = t("youtube.quality.prompt");
    let entities = entities_for_text(&text);
    let mut params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(quality_keyboard(
            &options,
        )))
        .build();

    if !entities.is_empty() {
        params.entities = Some(entities);
    }

    api.send_message(&params).await?;
    log_trace(
        trace_id,
        "quality_prompt_sent",
        &format!("chat_id={chat_id}"),
    );
    Ok(())
}

pub async fn handle_quality_callback(api: &Bot, callback_query: &CallbackQuery) -> bool {
    let Some(data) = callback_query.data.as_deref() else {
        return false;
    };
    if data.starts_with(CB_QUALITY_PREFIX) {
        return handle_resolution_callback(api, callback_query, data).await;
    }
    if data.starts_with(CB_CODEC_PREFIX) {
        return handle_codec_callback(api, callback_query, data).await;
    }
    false
}

async fn handle_resolution_callback(api: &Bot, callback_query: &CallbackQuery, data: &str) -> bool {
    let Some(selection) = parse_resolution_callback(data) else {
        eprintln!(
            "[youtube callback event=quality_malformed user_id={} data={data}]",
            callback_query.from.id
        );
        answer_callback(api, callback_query, "youtube.quality.not_ready").await;
        return true;
    };

    eprintln!(
        "[youtube callback event=quality_clicked user_id={} height={} codecs={}]",
        callback_query.from.id,
        selection.height,
        selection
            .codecs
            .iter()
            .map(|codec| codec.key())
            .collect::<Vec<_>>()
            .join(",")
    );

    if selection.codecs.len() <= 1 {
        answer_callback(api, callback_query, "youtube.quality.not_ready").await;
        return true;
    }

    let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message.as_ref() else {
        answer_callback(api, callback_query, "youtube.quality.not_ready").await;
        return true;
    };

    let quality_label = quality_label(selection.height);
    let text = tf("youtube.codec.prompt", &[("quality", &quality_label)]);
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(message.chat.id)
        .message_id(message.message_id)
        .text(text)
        .reply_markup(codec_keyboard(selection.height, &selection.codecs))
        .build();

    if !entities.is_empty() {
        params.entities = Some(entities);
    }

    if let Err(error) = api.edit_message_text(&params).await {
        eprintln!(
            "[youtube callback event=codec_prompt_edit_failed user_id={} height={} error={error}]",
            callback_query.from.id, selection.height
        );
        answer_callback(api, callback_query, "youtube.quality.not_ready").await;
        return true;
    }

    eprintln!(
        "[youtube callback event=codec_prompt_sent user_id={} height={} codecs={}]",
        callback_query.from.id,
        selection.height,
        selection
            .codecs
            .iter()
            .map(|codec| codec.key())
            .collect::<Vec<_>>()
            .join(",")
    );
    answer_callback(api, callback_query, "").await;
    true
}

async fn handle_codec_callback(api: &Bot, callback_query: &CallbackQuery, data: &str) -> bool {
    let Some((height, codec)) = parse_codec_callback(data) else {
        eprintln!(
            "[youtube callback event=codec_malformed user_id={} data={data}]",
            callback_query.from.id
        );
        answer_callback(api, callback_query, "youtube.quality.not_ready").await;
        return true;
    };

    eprintln!(
        "[youtube callback event=codec_clicked user_id={} height={} codec={}]",
        callback_query.from.id,
        height,
        codec.key()
    );
    answer_callback(api, callback_query, "youtube.quality.not_ready").await;
    true
}

async fn answer_callback(api: &Bot, callback_query: &CallbackQuery, text_key: &str) {
    let mut params = AnswerCallbackQueryParams::builder()
        .callback_query_id(&callback_query.id)
        .build();
    if !text_key.is_empty() {
        params.text = Some(t(text_key));
    }
    let _ = api.answer_callback_query(&params).await;
}

fn quality_keyboard(options: &[QualityOption]) -> InlineKeyboardMarkup {
    let rows = options
        .iter()
        .map(|option| {
            vec![quality_button(
                &t(option.label_key),
                &quality_callback_data(option),
                button_style(option.height),
            )]
        })
        .collect();

    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

fn codec_keyboard(height: u32, codecs: &[VideoCodec]) -> InlineKeyboardMarkup {
    let rows = codecs
        .iter()
        .map(|codec| {
            vec![quality_button(
                &t(codec.label_key()),
                &format!("{CB_CODEC_PREFIX}{height}:{}", codec.key()),
                ButtonStyle::Primary,
            )]
        })
        .collect();

    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

fn quality_options(info: &VideoInfo) -> Vec<QualityOption> {
    QUALITY_OPTIONS
        .iter()
        .filter_map(|(height, label_key)| {
            let codecs = codecs_for_height(info, *height);
            if codecs.is_empty() {
                return None;
            }
            Some(QualityOption {
                height: *height,
                label_key,
                codecs,
            })
        })
        .collect()
}

fn codecs_for_height(info: &VideoInfo, height: u32) -> Vec<VideoCodec> {
    CODEC_ORDER
        .iter()
        .copied()
        .filter(|codec| {
            info.video_formats
                .iter()
                .any(|format| format.height == height && format.codec == *codec)
        })
        .collect()
}

fn parse_resolution_callback(data: &str) -> Option<QualitySelection> {
    let rest = data.strip_prefix(CB_QUALITY_PREFIX)?;
    let (height, codecs) = rest.split_once(':')?;
    let height = height.parse().ok()?;
    let codecs = codecs
        .split(',')
        .filter_map(VideoCodec::from_key)
        .collect::<Vec<_>>();
    if codecs.is_empty() {
        return None;
    }
    Some(QualitySelection { height, codecs })
}

fn parse_codec_callback(data: &str) -> Option<(u32, VideoCodec)> {
    let rest = data.strip_prefix(CB_CODEC_PREFIX)?;
    let (height, codec) = rest.split_once(':')?;
    Some((height.parse().ok()?, VideoCodec::from_key(codec)?))
}

fn quality_callback_data(option: &QualityOption) -> String {
    let codecs = option
        .codecs
        .iter()
        .map(|codec| codec.key())
        .collect::<Vec<_>>()
        .join(",");
    format!("{CB_QUALITY_PREFIX}{}:{codecs}", option.height)
}

fn quality_label(height: u32) -> String {
    QUALITY_OPTIONS
        .iter()
        .find(|(option_height, _)| *option_height == height)
        .map(|(_, label_key)| t(label_key))
        .unwrap_or_else(|| format!("{height}p"))
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

fn format_summary(video_formats: &[VideoFormatOption]) -> String {
    video_formats
        .iter()
        .map(|format| format!("{}:{}", format.height, format.codec.key()))
        .collect::<Vec<_>>()
        .join(",")
}

struct QualityOption {
    height: u32,
    label_key: &'static str,
    codecs: Vec<VideoCodec>,
}

struct QualitySelection {
    height: u32,
    codecs: Vec<VideoCodec>,
}
