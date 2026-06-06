use frankenstein::{client_reqwest::Bot, types::CallbackQuery};

use crate::i18n::{entities_for_text, t, tf};

use super::super::download::{
    Selection, SelectionView, SubtitleMode, YoutubeRequest, get_request, spawn_download, with_selection,
};
use super::super::trace::log_trace;
use super::super::types::VideoCodec;
use super::buttons::{answer, quality_label};
use super::constants::*;
use super::panel::{extract_message, refresh_full_panel, refresh_keyboard};
use frankenstein::{AsyncTelegramApi, methods::EditMessageTextParams};

pub async fn handle_selection_callback(api: &Bot, callback_query: &CallbackQuery) -> bool {
    let Some(data) = callback_query.data.as_deref() else { return false; };
    if !data.starts_with(CB_SELECTION_PREFIX) { return false; }

    if data == CB_NOP {
        answer(api, callback_query, "").await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_CODEC) {
        handle_codec_toggle(api, callback_query, rest).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_AUDIO) {
        handle_audio_toggle(api, callback_query, rest).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_SUB_TOGGLE) {
        handle_sub_toggle(api, callback_query, rest).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_SUB_MENU) {
        handle_sub_view_change(api, callback_query, rest, Some(0)).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_SUB_BACK) {
        handle_sub_view_change(api, callback_query, rest, None).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_SUB_PAGE) {
        handle_sub_page(api, callback_query, rest).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_SUB_MODE) {
        handle_sub_mode_toggle(api, callback_query, rest).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_GO) {
        handle_go(api, callback_query, rest).await;
        return true;
    }
    answer(api, callback_query, "").await;
    true
}

async fn parse_rid_and_req<'a>(
    api: &Bot,
    cq: &CallbackQuery,
    rest: &str,
) -> Option<(u64, YoutubeRequest)> {
    let request_id = rest.parse::<u64>().ok()?;
    let req = get_request(request_id);
    if req.is_none() {
        answer(api, cq, "youtube.download.request_expired").await;
    }
    req.map(|r| (request_id, r))
}

async fn handle_codec_toggle(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some((request_id, codec_key)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let (Ok(request_id), Some(codec)) = (request_id.parse::<u64>(), VideoCodec::from_key(codec_key)) else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let trace_id = req.trace_id;
    let changed = with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut() {
            if sel.codec != codec { sel.codec = codec; return true; }
        }
        false
    });
    log_trace(trace_id, "selection_codec", &format!("request_id={request_id} codec={} changed={changed}", codec.key()));
    if changed {
        if let Some(msg) = extract_message(cq) {
            refresh_full_panel(api, msg, &req, request_id).await;
        }
    }
    answer(api, cq, "").await;
}

async fn handle_audio_toggle(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some((request_id, idx_str)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let (Ok(request_id), Ok(idx)) = (request_id.parse::<u64>(), idx_str.parse::<usize>()) else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let Some(lang) = req.audio_languages.get(idx).map(|l| l.code.clone()) else {
        answer(api, cq, "").await;
        return;
    };
    let trace_id = req.trace_id;
    let changed = with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut() {
            if sel.audio_lang.as_deref() != Some(&lang) { sel.audio_lang = Some(lang.clone()); return true; }
        }
        false
    });
    log_trace(trace_id, "selection_audio", &format!("request_id={request_id} lang={lang} changed={changed}"));
    if changed {
        if let Some(msg) = extract_message(cq) {
            refresh_keyboard(api, msg, &req, request_id).await;
        }
    }
    answer(api, cq, "").await;
}

async fn handle_sub_toggle(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some((request_id, idx_str)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let (Ok(request_id), Ok(idx)) = (request_id.parse::<u64>(), idx_str.parse::<usize>()) else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let Some(lang) = req.subtitle_languages.get(idx).map(|l| l.code.clone()) else {
        answer(api, cq, "").await;
        return;
    };
    let trace_id = req.trace_id;
    let (added, total) = with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut() {
            if let Some(pos) = sel.subtitle_langs.iter().position(|l| l == &lang) {
                sel.subtitle_langs.remove(pos);
                (false, sel.subtitle_langs.len())
            } else {
                sel.subtitle_langs.push(lang.clone());
                (true, sel.subtitle_langs.len())
            }
        } else { (false, 0) }
    });
    log_trace(trace_id, "selection_sub_toggle", &format!("request_id={request_id} lang={lang} added={added} total_selected={total}"));
    if let Some(msg) = extract_message(cq) {
        refresh_keyboard(api, msg, &req, request_id).await;
    }
    answer(api, cq, "").await;
}

async fn handle_sub_mode_toggle(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some((request_id, mode_str)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let Ok(request_id) = request_id.parse::<u64>() else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let new_mode = match mode_str {
        "file" => SubtitleMode::File,
        "embedded" => SubtitleMode::Embedded,
        _ => { answer(api, cq, "").await; return; }
    };
    let trace_id = req.trace_id;
    let changed = with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut() {
            if sel.subtitle_mode != new_mode { sel.subtitle_mode = new_mode; return true; }
        }
        false
    });
    log_trace(trace_id, "selection_sub_mode", &format!("request_id={request_id} mode={mode_str} changed={changed}"));
    if changed {
        if let Some(msg) = extract_message(cq) {
            refresh_keyboard(api, msg, &req, request_id).await;
        }
    }
    answer(api, cq, "").await;
}

async fn handle_sub_view_change(api: &Bot, cq: &CallbackQuery, rest: &str, page: Option<usize>) {
    let Some((request_id, req)) = parse_rid_and_req(api, cq, rest).await else { return; };
    let trace_id = req.trace_id;
    with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut() {
            sel.view = match page {
                Some(p) => SelectionView::SubMenu(p),
                None => SelectionView::Main,
            };
        }
    });
    log_trace(trace_id, "selection_view", &format!(
        "request_id={request_id} view={}", match page { Some(p) => format!("submenu:{p}"), None => "main".to_string() }
    ));
    if let Some(msg) = extract_message(cq) {
        refresh_keyboard(api, msg, &req, request_id).await;
    }
    answer(api, cq, "").await;
}

async fn handle_sub_page(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some((request_id, page_str)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let (Ok(request_id), Ok(page)) = (request_id.parse::<u64>(), page_str.parse::<usize>()) else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let trace_id = req.trace_id;
    with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut() { sel.view = SelectionView::SubMenu(page); }
    });
    log_trace(trace_id, "selection_sub_page", &format!("request_id={request_id} page={page}"));
    if let Some(msg) = extract_message(cq) {
        refresh_keyboard(api, msg, &req, request_id).await;
    }
    answer(api, cq, "").await;
}

async fn handle_go(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Ok(request_id) = rest.parse::<u64>() else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let Some(message) = extract_message(cq) else {
        answer(api, cq, "").await;
        return;
    };
    let trace_id = req.trace_id;
    let selection = with_selection(&req, |slot| slot.clone()).unwrap_or_else(|| {
        log_trace(trace_id, "selection_go_missing", "no selection present, falling back");
        Selection {
            height: req.formats.first().map(|f| f.height).unwrap_or(720),
            codec: req.formats.first().map(|f| f.codec).unwrap_or(VideoCodec::H264),
            audio_lang: None,
            subtitle_langs: Vec::new(),
            subtitle_mode: SubtitleMode::Embedded,
            view: SelectionView::Main,
        }
    });
    log_trace(trace_id, "selection_confirm", &format!(
        "request_id={request_id} height={} codec={} audio={:?} subs={:?}",
        selection.height, selection.codec.key(), selection.audio_lang, selection.subtitle_langs
    ));
    answer(api, cq, "").await;
    let quality_lbl = quality_label(selection.height);
    let text = tf("youtube.download.starting", &[("quality", &quality_lbl)]);
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(message.chat.id)
        .message_id(message.message_id)
        .text(text)
        .build();
    if !entities.is_empty() { params.entities = Some(entities); }
    if let Err(e) = api.edit_message_text(&params).await {
        log_trace(trace_id, "selection_start_edit_failed", &e.to_string());
    }
    spawn_download(api.clone(), request_id, selection, message.chat.id, message.message_id);
}
