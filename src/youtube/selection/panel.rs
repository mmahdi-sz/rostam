use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{EditMessageReplyMarkupParams, EditMessageTextParams},
    types::{MaybeInaccessibleMessage, MessageEntity, MessageEntityType},
};

use crate::i18n::{entities_for_text, t, tf};

use super::super::download::{YoutubeRequest, get_request, init_selection, with_selection};
use super::super::trace::log_trace;
use super::buttons::quality_label;
use super::keyboard::build_keyboard;

pub async fn enter_selection_menu(api: &Bot, request_id: u64, height: u32, chat_id: i64, message_id: i32) {
    let Some(req) = get_request(request_id) else { return; };
    let trace_id = req.trace_id;
    let selection = init_selection(&req, height);
    log_trace(
        trace_id,
        "selection_open",
        &format!(
            "request_id={request_id} height={height} codec={} audio={:?} subs_avail={} audio_avail={}",
            selection.codec.key(),
            selection.audio_lang,
            req.subtitle_languages.len(),
            req.audio_languages.len()
        ),
    );
    with_selection(&req, |slot| { *slot = Some(selection); });

    let sel = with_selection(&req, |slot| slot.clone()).unwrap();
    let (text, entities) = build_selection_text(&req, &sel);
    let keyboard = build_keyboard(&req, request_id);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .reply_markup(keyboard)
        .build();
    if !entities.is_empty() { params.entities = Some(entities); }
    if let Err(e) = api.edit_message_text(&params).await {
        log_trace(trace_id, "selection_open_edit_failed", &e.to_string());
    }
}

pub fn build_selection_text(req: &YoutubeRequest, sel: &super::super::download::Selection) -> (String, Vec<MessageEntity>) {
    let prompt_header = t("youtube.selection.prompt");
    let codec_desc = t("youtube.selection.codec_description");
    let quality = quality_label(sel.height);
    let codec_name = t(sel.codec.label_key());

    let (bitrate_str, size_str) = req
        .formats
        .iter()
        .find(|f| f.height == sel.height && f.codec == sel.codec)
        .map(|f| {
            let br = f.bitrate.map(|b| format!("{:.0}", b)).unwrap_or_else(|| "?".to_string());
            let sz = f.bitrate.and_then(|b| {
                req.duration.map(|dur| format!("{:.0}", b * 1000.0 / 8.0 * dur as f64 / (1024.0 * 1024.0)))
            }).unwrap_or_else(|| "?".to_string());
            (br, sz)
        })
        .unwrap_or_else(|| ("?".to_string(), "?".to_string()));

    let video_info = tf(
        "youtube.selection.video_info",
        &[("quality", &quality), ("codec", &codec_name), ("size", &size_str), ("bitrate", &bitrate_str)],
    );

    let full = format!("{prompt_header}\n{codec_desc}\n\n{video_info}");
    let mut entities = entities_for_text(&full);
    let blockquote_offset = prompt_header.encode_utf16().count() + 1;
    let blockquote_length = codec_desc.encode_utf16().count();
    entities.push(MessageEntity {
        type_field: MessageEntityType::ExpandableBlockquote,
        offset: blockquote_offset as u16,
        length: blockquote_length as u16,
        url: None, user: None, language: None,
        custom_emoji_id: None, unix_time: None, date_time_format: None,
    });
    (full, entities)
}

pub async fn refresh_keyboard(api: &Bot, message: &frankenstein::types::Message, req: &YoutubeRequest, request_id: u64) {
    let keyboard = build_keyboard(req, request_id);
    let params = EditMessageReplyMarkupParams::builder()
        .chat_id(message.chat.id)
        .message_id(message.message_id)
        .reply_markup(keyboard)
        .build();
    if let Err(e) = api.edit_message_reply_markup(&params).await {
        let desc = e.to_string();
        if !desc.contains("message is not modified") {
            log_trace(req.trace_id, "selection_refresh_failed", &desc);
        }
    }
}

pub async fn refresh_full_panel(api: &Bot, message: &frankenstein::types::Message, req: &YoutubeRequest, request_id: u64) {
    let sel = with_selection(req, |slot| slot.clone()).unwrap();
    let (text, entities) = build_selection_text(req, &sel);
    let keyboard = build_keyboard(req, request_id);
    let mut params = EditMessageTextParams::builder()
        .chat_id(message.chat.id)
        .message_id(message.message_id)
        .text(text)
        .reply_markup(keyboard)
        .build();
    if !entities.is_empty() { params.entities = Some(entities); }
    if let Err(e) = api.edit_message_text(&params).await {
        let desc = e.to_string();
        if !desc.contains("message is not modified") {
            log_trace(req.trace_id, "selection_refresh_failed", &desc);
        }
    }
}

pub fn extract_message(cq: &frankenstein::types::CallbackQuery) -> Option<&frankenstein::types::Message> {
    match cq.message.as_ref()? {
        MaybeInaccessibleMessage::Message(m) => Some(m),
        _ => None,
    }
}
