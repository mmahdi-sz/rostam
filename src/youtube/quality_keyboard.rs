use std::sync::{Arc, Mutex};

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{AnswerCallbackQueryParams, EditMessageTextParams, SendMessageParams},
    types::{
        ButtonStyle, CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup,
        MaybeInaccessibleMessage, ReplyMarkup,
    },
};

use crate::database::postgresql::PostgresDatabase;
use crate::i18n::{apply_premium_to_md, t};
use crate::rank;

use super::download::types::AudioQuality;
use super::download::{YoutubeRequest, cancel_download, get_request, store_request};
use super::selection::{
    CB_BACK_TO_QUALITY_PREFIX, CB_SELECTION_PREFIX, enter_selection_menu, handle_selection_callback,
};
use super::trace::log_trace;
use super::types::{VideoCodec, VideoFormatOption, VideoInfo};
use crate::bot::CB_START_PANEL;

const CB_QUALITY_PREFIX: &str = "yt:q:";
const CB_CODEC_PREFIX: &str = "yt:c:";
const CB_CANCEL_PREFIX: &str = "yt:cancel:";
const CB_AUDIO_PREFIX: &str = "yt:audio:";

const QUALITY_OPTIONS: &[(u32, &str, &str)] = &[
    (4320, "youtube.quality.buttons.4320", "diamond"),
    (2160, "youtube.quality.buttons.2160", "diamond"),
    (1440, "youtube.quality.buttons.1440", "fire_yt"),
    (1080, "youtube.quality.buttons.1080", "sparkles"),
    (720, "youtube.quality.buttons.720", "star_yt"),
    (480, "youtube.quality.buttons.480", "phone"),
    (360, "youtube.quality.buttons.360", "signal"),
    (240, "youtube.quality.buttons.240", "signal"),
    (144, "youtube.quality.buttons.144", "signal"),
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
) -> crate::error::Result<()> {
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
        // Never fail silently: tell the user no quality is downloadable.
        let _ = crate::bot::send_text(api, chat_id, &t("youtube.quality.no_formats")).await;
        return Ok(());
    }

    let request = YoutubeRequest {
        trace_id,
        chat_id,
        user_id,
        webpage_url: info.webpage_url.clone(),
        cookie_spec: cookie_spec.to_string(),
        title: info.title.clone(),
        channel: info.channel.clone(),
        duration: info.duration,
        thumbnail_url: info.thumbnail.clone(),
        formats: info.video_formats.clone(),
        audio_languages: info.audio_languages.clone(),
        subtitle_languages: info.subtitle_languages.clone(),
        selection: Arc::new(Mutex::new(None)),
        is_playlist: info.is_playlist,
        playlist_items: info.playlist_items.clone(),
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
    let raw = if info.is_playlist {
        let count = info.playlist_item_count.unwrap_or(0);
        format!(
            "{}\n\n{}",
            t("youtube.quality.prompt"),
            crate::i18n::tf(
                "youtube.quality.playlist_suffix",
                &[("count", &crate::i18n::to_fa_digits(&count.to_string()))]
            )
        )
    } else {
        t("youtube.quality.prompt")
    };
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

pub async fn handle_quality_callback(
    api: &Bot,
    callback_query: &CallbackQuery,
    database: &Option<PostgresDatabase>,
) -> bool {
    let Some(data) = callback_query.data.as_deref() else {
        return false;
    };
    if data.starts_with(CB_SELECTION_PREFIX) {
        return handle_selection_callback(api, callback_query, database).await;
    }
    if data.starts_with(CB_QUALITY_PREFIX) {
        return handle_resolution_callback(api, callback_query, data, database).await;
    }
    if data.starts_with(CB_CODEC_PREFIX) {
        // legacy stale callback (no longer issued); just ack
        answer_callback(api, callback_query, "").await;
        return true;
    }
    if data.starts_with(CB_CANCEL_PREFIX) {
        return handle_cancel_callback(api, callback_query, data).await;
    }
    if data.starts_with(CB_BACK_TO_QUALITY_PREFIX) {
        return handle_back_to_quality_callback(api, callback_query, data).await;
    }
    if data.starts_with(CB_AUDIO_PREFIX) {
        return handle_audio_quality_callback(api, callback_query, data).await;
    }
    false
}

async fn handle_resolution_callback(
    api: &Bot,
    callback_query: &CallbackQuery,
    data: &str,
    database: &Option<PostgresDatabase>,
) -> bool {
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

    log_actor_id!("yt", trace_id, callback_query.from.id as i64, "clicked" => format!("yt:q:{request_id}:{height}"));
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

    // rank check — live از db
    if let (Some(uid), Some(db)) = (request.user_id, database.as_ref()) {
        let user_rank = rank::effective_rank(db.client(), uid).await;
        if let Some(max) = user_rank.max_yt_quality()
            && height > max
        {
            log_trace(
                trace_id,
                "quality_paywall",
                &format!(
                    "user_id={uid} height={height} max={max} rank={}",
                    user_rank.as_str()
                ),
            );
            answer_callback(api, callback_query, "").await;
            let limit = format!("{max}p");
            let min_rank = rank::types::Rank::min_for_quality(height);
            crate::rank::paywall::block_limit(api, message.chat.id, &limit, min_rank).await;
            return true;
        }
    }

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
    let mut rows: Vec<Vec<InlineKeyboardButton>> = options
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

    rows.push(vec![quality_button(
        &t("youtube.audio.quality.best"),
        &format!("{CB_AUDIO_PREFIX}{request_id}:best"),
        ButtonStyle::Success,
        "music_note",
    )]);
    rows.push(vec![quality_button(
        &t("youtube.audio.quality.medium"),
        &format!("{CB_AUDIO_PREFIX}{request_id}:mid"),
        ButtonStyle::Primary,
        "music_note",
    )]);
    rows.push(vec![quality_button(
        &t("youtube.audio.quality.low"),
        &format!("{CB_AUDIO_PREFIX}{request_id}:low"),
        ButtonStyle::Danger,
        "music_note",
    )]);
    rows.push(vec![main_menu_button()]);

    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

fn main_menu_button() -> InlineKeyboardButton {
    let icon_id = t("emoji.panel.icons.back");
    InlineKeyboardButton {
        text: t("start.back"),
        icon_custom_emoji_id: if icon_id.is_empty() || icon_id.starts_with('!') {
            None
        } else {
            Some(icon_id)
        },
        callback_data: Some(CB_START_PANEL.to_string()),
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

fn quality_button(
    text: &str,
    callback_data: &str,
    style: ButtonStyle,
    icon_key: &str,
) -> InlineKeyboardButton {
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

async fn handle_audio_quality_callback(api: &Bot, cq: &CallbackQuery, data: &str) -> bool {
    let rest = data.strip_prefix(CB_AUDIO_PREFIX).unwrap_or("");
    let Some((req_str, quality_str)) = rest.split_once(':') else {
        answer_callback(api, cq, "").await;
        return true;
    };
    let Some(request_id) = req_str.parse::<u64>().ok() else {
        answer_callback(api, cq, "").await;
        return true;
    };
    let Some(audio_quality) = AudioQuality::from_str(quality_str) else {
        answer_callback(api, cq, "").await;
        return true;
    };
    let Some(MaybeInaccessibleMessage::Message(message)) = cq.message.as_ref() else {
        answer_callback(api, cq, "").await;
        return true;
    };
    if get_request(request_id).is_none() {
        answer_callback(api, cq, "youtube.download.request_expired").await;
        return true;
    }

    use super::download::spawn_download;
    use super::download::types::{Selection, SelectionView, SubtitleMode};
    use crate::i18n::tf;

    let selection = Selection {
        height: 0,
        codec: super::types::VideoCodec::H264,
        audio_lang: None,
        subtitle_langs: Vec::new(),
        subtitle_mode: SubtitleMode::Embedded,
        view: SelectionView::Main,
        audio_only: Some(audio_quality),
    };

    let status_text = tf(
        "youtube.audio.starting",
        &[("quality", &t(audio_quality.label_key()))],
    );
    let edit_result = api
        .edit_message_text(
            &EditMessageTextParams::builder()
                .chat_id(message.chat.id)
                .message_id(message.message_id)
                .text(&status_text)
                .build(),
        )
        .await;

    if edit_result.is_ok() {
        spawn_download(
            api.clone(),
            request_id,
            selection,
            message.chat.id,
            message.message_id,
        );
        log_trace(
            0,
            "audio_quality_dispatch",
            &format!("request_id={request_id} quality={quality_str}"),
        );
    }

    answer_callback(api, cq, "").await;
    true
}

async fn handle_back_to_quality_callback(api: &Bot, cq: &CallbackQuery, data: &str) -> bool {
    let rest = data.strip_prefix(CB_BACK_TO_QUALITY_PREFIX).unwrap_or("");
    let Some(request_id) = rest.parse::<u64>().ok() else {
        answer_callback(api, cq, "").await;
        return true;
    };
    let Some(req) = get_request(request_id) else {
        answer_callback(api, cq, "youtube.download.request_expired").await;
        return true;
    };
    let Some(MaybeInaccessibleMessage::Message(message)) = cq.message.as_ref() else {
        answer_callback(api, cq, "").await;
        return true;
    };
    let options: Vec<QualityOption> = QUALITY_OPTIONS
        .iter()
        .filter_map(|(height, label_key, icon_key)| {
            // back-to-quality: max_quality نداریم، همون که اول نشون داده شد کافیه
            let codecs: Vec<super::types::VideoCodec> = CODEC_ORDER
                .iter()
                .copied()
                .filter(|codec| {
                    req.formats
                        .iter()
                        .any(|f| f.height == *height && f.codec == *codec)
                })
                .collect();
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
        .collect();
    let raw = t("youtube.quality.prompt");
    let text = apply_premium_to_md(&raw);
    log_trace(
        req.trace_id,
        "back_to_quality",
        &format!("request_id={request_id} user_id={}", cq.from.id),
    );
    let params = EditMessageTextParams::builder()
        .chat_id(message.chat.id)
        .message_id(message.message_id)
        .text(text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(quality_keyboard(request_id, &options))
        .build();
    if let Err(e) = api.edit_message_text(&params).await {
        log_trace(req.trace_id, "back_to_quality_edit_failed", &e.to_string());
    }
    answer_callback(api, cq, "").await;
    true
}

async fn handle_cancel_callback(api: &Bot, callback_query: &CallbackQuery, data: &str) -> bool {
    let rest = data.strip_prefix(CB_CANCEL_PREFIX).unwrap_or("");
    let Some(request_id) = rest.parse::<u64>().ok() else {
        answer_callback(api, callback_query, "").await;
        return true;
    };
    let cancelled = cancel_download(request_id);
    log_trace(
        0,
        "cancel_callback",
        &format!(
            "request_id={request_id} cancelled={cancelled} user_id={}",
            callback_query.from.id
        ),
    );
    crate::stats::record_event_user(
        callback_query.from.id as i64,
        "youtube",
        "cancel",
        if cancelled { "ok" } else { "not_found" },
        0,
    )
    .await;
    answer_callback(api, callback_query, "").await;
    true
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
