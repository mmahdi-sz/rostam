use std::sync::{Arc, Mutex};

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{AnswerCallbackQueryParams, SendMessageParams},
    types::{
        ButtonStyle, CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup,
        MaybeInaccessibleMessage, ReplyMarkup,
    },
};

use crate::i18n::{apply_premium_to_md, t};

use super::download::{YoutubeRequest, get_request, store_request};
use super::selection::{enter_selection_menu, handle_selection_callback, CB_SELECTION_PREFIX};
use super::trace::log_trace;
use super::types::{VideoCodec, VideoFormatOption, VideoInfo};

const CB_QUALITY_PREFIX: &str = "yt:q:";
const CB_CODEC_PREFIX: &str = "yt:c:";

const QUALITY_OPTIONS: &[(u32, &str, &str)] = &[
    (4320, "youtube.quality.buttons.4320", "diamond"),
    (2160, "youtube.quality.buttons.2160", "diamond"),
    (1440, "youtube.quality.buttons.1440", "fire_yt"),
    (1080, "youtube.quality.buttons.1080", "sparkles"),
    (720,  "youtube.quality.buttons.720",  "star_yt"),
    (480,  "youtube.quality.buttons.480",  "phone"),
    (360,  "youtube.quality.buttons.360",  "signal"),
    (240,  "youtube.quality.buttons.240",  "signal"),
    (144,  "youtube.quality.buttons.144",  "signal"),
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
    user_id: Option<i64>,
    cookie_spec: &str,
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

    let request = YoutubeRequest {
        trace_id,
        chat_id,
        user_id,
        webpage_url: info.webpage_url.clone(),
        cookie_spec: cookie_spec.to_string(),
        title: info.title.clone(),
        duration: info.duration,
        thumbnail_url: info.thumbnail.clone(),
        formats: info.video_formats.clone(),
        audio_languages: info.audio_languages.clone(),
        subtitle_languages: info.subtitle_languages.clone(),
        selection: Arc::new(Mutex::new(None)),
    };
    let request_id = store_request(request);

    let button_summary = options
        .iter()
        .map(|option| format!("{}:{:?}", option.height, option.codecs))
        .collect::<Vec<_>>()
        .join(",");
    log_trace(
        trace_id,
        "quality_prompt_buttons",
        &format!(
            "request_id={request_id} available_heights={:?} buttons={button_summary}",
            info.available_heights
        ),
    );
    let raw = t("youtube.quality.prompt");
    let text = apply_premium_to_md(&raw);
    api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(quality_keyboard(
                request_id, &options,
            )))
            .build(),
    )
    .await?;
    log_trace(
        trace_id,
        "quality_prompt_sent",
        &format!("chat_id={chat_id} request_id={request_id}"),
    );
    Ok(())
}

pub async fn handle_quality_callback(api: &Bot, callback_query: &CallbackQuery) -> bool {
    let Some(data) = callback_query.data.as_deref() else {
        return false;
    };
    if data.starts_with(CB_SELECTION_PREFIX) {
        return handle_selection_callback(api, callback_query).await;
    }
    if data.starts_with(CB_QUALITY_PREFIX) {
        return handle_resolution_callback(api, callback_query, data).await;
    }
    if data.starts_with(CB_CODEC_PREFIX) {
        // legacy stale callback (no longer issued); just ack
        answer_callback(api, callback_query, "").await;
        return true;
    }
    false
}

async fn handle_resolution_callback(api: &Bot, callback_query: &CallbackQuery, data: &str) -> bool {
    let Some((request_id, height)) = parse_quality_callback(data) else {
        eprintln!(
            "[youtube callback event=quality_malformed user_id={} data={data}]",
            callback_query.from.id
        );
        answer_callback(api, callback_query, "youtube.download.request_expired").await;
        return true;
    };

    let Some(request) = get_request(request_id) else {
        log_trace(
            0,
            "quality_request_missing",
            &format!("request_id={request_id} height={height}"),
        );
        answer_callback(api, callback_query, "youtube.download.request_expired").await;
        return true;
    };
    let trace_id = request.trace_id;

    log_trace(
        trace_id,
        "quality_clicked",
        &format!(
            "request_id={request_id} user_id={} height={}",
            callback_query.from.id, height
        ),
    );

    let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message.as_ref() else {
        answer_callback(api, callback_query, "youtube.download.request_expired").await;
        return true;
    };

    enter_selection_menu(api, request_id, height, message.chat.id, message.message_id).await;
    answer_callback(api, callback_query, "").await;
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

fn quality_keyboard(request_id: u64, options: &[QualityOption]) -> InlineKeyboardMarkup {
    let rows = options
        .iter()
        .map(|option| {
            vec![quality_button(
                &t(option.label_key),
                &format!("{CB_QUALITY_PREFIX}{request_id}:{}", option.height),
                button_style(option.height),
                option.icon_key,
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
        .filter_map(|(height, label_key, icon_key)| {
            let codecs = codecs_for_height(info, *height);
            if codecs.is_empty() {
                return None;
            }
            Some(QualityOption {
                height: *height,
                label_key,
                icon_key,
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

fn parse_quality_callback(data: &str) -> Option<(u64, u32)> {
    let rest = data.strip_prefix(CB_QUALITY_PREFIX)?;
    let (req, height) = rest.split_once(':')?;
    Some((req.parse().ok()?, height.parse().ok()?))
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

fn quality_button(text: &str, callback_data: &str, style: ButtonStyle, icon_key: &str) -> InlineKeyboardButton {
    let icon_id = t(&format!("emoji.panel.icons.{icon_key}"));
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
    icon_key: &'static str,
    codecs: Vec<VideoCodec>,
}
