use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{AnswerCallbackQueryParams, EditMessageReplyMarkupParams, EditMessageTextParams},
    types::{
        ButtonStyle, CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup,
        MaybeInaccessibleMessage,
    },
};

use frankenstein::types::MessageEntityType;

use crate::i18n::{entities_for_text, t, tf};

use super::download::{
    Selection, SelectionView, YoutubeRequest, codecs_for_height, get_request, init_selection,
    spawn_download, with_selection,
};
use super::lang_names::lang_name_fa;
use super::trace::log_trace;
use super::types::VideoCodec;

pub const CB_SELECTION_PREFIX: &str = "yt:s:";
pub const CB_BACK_TO_QUALITY_PREFIX: &str = "yt:bk:";
const CB_NOP: &str = "yt:s:nop";
const CB_CODEC: &str = "yt:s:c:";
const CB_AUDIO: &str = "yt:s:a:";
const CB_SUB_TOGGLE: &str = "yt:s:t:";
const CB_SUB_MENU: &str = "yt:s:sm:";
const CB_SUB_BACK: &str = "yt:s:sb:";
const CB_SUB_PAGE: &str = "yt:s:sp:";
const CB_GO: &str = "yt:s:go:";

const CODEC_DISPLAY_ORDER: &[VideoCodec] = &[
    VideoCodec::Av1,
    VideoCodec::Vp9,
    VideoCodec::H265,
    VideoCodec::H264,
];

const SUB_PAGE_ROWS: usize = 4;
const SUB_PAGE_COLS: usize = 2;
const SUB_PER_PAGE: usize = SUB_PAGE_ROWS * SUB_PAGE_COLS;

pub async fn enter_selection_menu(
    api: &Bot,
    request_id: u64,
    height: u32,
    chat_id: i64,
    message_id: i32,
) {
    let Some(req) = get_request(request_id) else {
        return;
    };
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
    with_selection(&req, |slot| {
        *slot = Some(selection);
    });

    let prompt_header = t("youtube.selection.prompt");
    let codec_desc = t("youtube.selection.codec_description");
    let prompt_raw = format!("{prompt_header}\n{codec_desc}");
    let mut entities = entities_for_text(&prompt_raw);
    let blockquote_offset = prompt_header.encode_utf16().count() + 1; // +1 for \n
    let blockquote_length = codec_desc.encode_utf16().count();
    entities.push(frankenstein::types::MessageEntity {
        type_field: MessageEntityType::ExpandableBlockquote,
        offset: blockquote_offset as u16,
        length: blockquote_length as u16,
        url: None,
        user: None,
        language: None,
        custom_emoji_id: None,
        unix_time: None,
        date_time_format: None,
    });
    let keyboard = build_keyboard(&req, request_id);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(prompt_raw.clone())
        .reply_markup(keyboard)
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    if let Err(e) = api.edit_message_text(&params).await {
        log_trace(trace_id, "selection_open_edit_failed", &e.to_string());
    }
}

pub async fn handle_selection_callback(api: &Bot, callback_query: &CallbackQuery) -> bool {
    let Some(data) = callback_query.data.as_deref() else {
        return false;
    };
    if !data.starts_with(CB_SELECTION_PREFIX) {
        return false;
    }
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
    if let Some(rest) = data.strip_prefix(CB_GO) {
        handle_go(api, callback_query, rest).await;
        return true;
    }
    answer(api, callback_query, "").await;
    true
}

async fn handle_codec_toggle(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some((request_id, codec_key)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let Some(request_id) = request_id.parse::<u64>().ok() else {
        answer(api, cq, "").await;
        return;
    };
    let Some(codec) = VideoCodec::from_key(codec_key) else {
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
            if sel.codec != codec {
                sel.codec = codec;
                return true;
            }
        }
        false
    });
    log_trace(
        trace_id,
        "selection_codec",
        &format!("request_id={request_id} codec={} changed={changed}", codec.key()),
    );
    if changed {
        refresh_keyboard(api, cq, &req, request_id).await;
    }
    answer(api, cq, "").await;
}

async fn handle_audio_toggle(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some((request_id, idx_str)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let Some(request_id) = request_id.parse::<u64>().ok() else {
        answer(api, cq, "").await;
        return;
    };
    let Some(idx) = idx_str.parse::<usize>().ok() else {
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
            if sel.audio_lang.as_deref() != Some(&lang) {
                sel.audio_lang = Some(lang.clone());
                return true;
            }
        }
        false
    });
    log_trace(
        trace_id,
        "selection_audio",
        &format!("request_id={request_id} lang={lang} changed={changed}"),
    );
    if changed {
        refresh_keyboard(api, cq, &req, request_id).await;
    }
    answer(api, cq, "").await;
}

async fn handle_sub_toggle(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some((request_id, idx_str)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let Some(request_id) = request_id.parse::<u64>().ok() else {
        answer(api, cq, "").await;
        return;
    };
    let Some(idx) = idx_str.parse::<usize>().ok() else {
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
        } else {
            (false, 0)
        }
    });
    log_trace(
        trace_id,
        "selection_sub_toggle",
        &format!("request_id={request_id} lang={lang} added={added} total_selected={total}"),
    );
    refresh_keyboard(api, cq, &req, request_id).await;
    answer(api, cq, "").await;
}

async fn handle_sub_view_change(
    api: &Bot,
    cq: &CallbackQuery,
    rest: &str,
    page: Option<usize>,
) {
    let Some(request_id) = rest.parse::<u64>().ok() else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let trace_id = req.trace_id;
    with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut() {
            sel.view = match page {
                Some(p) => SelectionView::SubMenu(p),
                None => SelectionView::Main,
            };
        }
    });
    log_trace(
        trace_id,
        "selection_view",
        &format!(
            "request_id={request_id} view={}",
            match page {
                Some(p) => format!("submenu:{p}"),
                None => "main".to_string(),
            }
        ),
    );
    refresh_keyboard(api, cq, &req, request_id).await;
    answer(api, cq, "").await;
}

async fn handle_sub_page(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some((request_id, page_str)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let Some(request_id) = request_id.parse::<u64>().ok() else {
        answer(api, cq, "").await;
        return;
    };
    let Some(page) = page_str.parse::<usize>().ok() else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let trace_id = req.trace_id;
    with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut() {
            sel.view = SelectionView::SubMenu(page);
        }
    });
    log_trace(
        trace_id,
        "selection_sub_page",
        &format!("request_id={request_id} page={page}"),
    );
    refresh_keyboard(api, cq, &req, request_id).await;
    answer(api, cq, "").await;
}

async fn handle_go(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some(request_id) = rest.parse::<u64>().ok() else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let Some(MaybeInaccessibleMessage::Message(message)) = cq.message.as_ref() else {
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
            view: SelectionView::Main,
        }
    });
    log_trace(
        trace_id,
        "selection_confirm",
        &format!(
            "request_id={request_id} height={} codec={} audio={:?} subs={:?}",
            selection.height,
            selection.codec.key(),
            selection.audio_lang,
            selection.subtitle_langs
        ),
    );
    answer(api, cq, "").await;
    let quality_label = quality_label(selection.height);
    let text = tf("youtube.download.starting", &[("quality", &quality_label)]);
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(message.chat.id)
        .message_id(message.message_id)
        .text(text)
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    if let Err(e) = api.edit_message_text(&params).await {
        log_trace(trace_id, "selection_start_edit_failed", &e.to_string());
    }
    spawn_download(
        api.clone(),
        request_id,
        selection.height,
        selection.codec,
        message.chat.id,
        message.message_id,
    );
}

async fn refresh_keyboard(api: &Bot, cq: &CallbackQuery, req: &YoutubeRequest, request_id: u64) {
    let Some(MaybeInaccessibleMessage::Message(message)) = cq.message.as_ref() else {
        return;
    };
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

fn build_keyboard(req: &YoutubeRequest, request_id: u64) -> InlineKeyboardMarkup {
    let selection = with_selection(req, |slot| slot.clone()).unwrap_or_else(|| Selection {
        height: 0,
        codec: VideoCodec::H264,
        audio_lang: None,
        subtitle_langs: Vec::new(),
        view: SelectionView::Main,
    });
    match selection.view {
        SelectionView::Main => build_main_keyboard(req, request_id, &selection),
        SelectionView::SubMenu(page) => build_sub_menu_keyboard(req, request_id, &selection, page),
    }
}

fn build_main_keyboard(
    req: &YoutubeRequest,
    request_id: u64,
    sel: &Selection,
) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    rows.push(vec![header_button(&t("youtube.selection.codec_header"))]);
    let codecs = codecs_for_height(req, sel.height);
    let codec_row: Vec<InlineKeyboardButton> = CODEC_DISPLAY_ORDER
        .iter()
        .filter(|c| codecs.contains(c))
        .map(|c| {
            let label = t(c.label_key());
            let selected = sel.codec == *c;
            choice_button(&label, format!("{CB_CODEC}{request_id}:{}", c.key()), selected)
        })
        .collect();
    if !codec_row.is_empty() {
        rows.push(codec_row);
    }

    if !req.audio_languages.is_empty() {
        rows.push(vec![header_button(&t("youtube.selection.audio_header"))]);
        let audio_row: Vec<InlineKeyboardButton> = req
            .audio_languages
            .iter()
            .enumerate()
            .map(|(i, lang)| {
                let mut label = lang_name_fa(&lang.code);
                if lang.is_original {
                    label.push_str(" ●");
                }
                let selected = sel.audio_lang.as_deref() == Some(&lang.code);
                choice_button(&label, format!("{CB_AUDIO}{request_id}:{i}"), selected)
            })
            .collect();
        for chunk in audio_row.chunks(3) {
            rows.push(chunk.to_vec());
        }
    }

    if !req.subtitle_languages.is_empty() {
        rows.push(vec![header_button(&t("youtube.selection.subtitle_header"))]);
        let mut quick_row: Vec<InlineKeyboardButton> = Vec::new();
        for quick_code in ["fa", "en"] {
            if let Some((i, lang)) = req
                .subtitle_languages
                .iter()
                .enumerate()
                .find(|(_, l)| l.code.eq_ignore_ascii_case(quick_code))
            {
                let label = lang_name_fa(&lang.code);
                let selected = sel.subtitle_langs.iter().any(|l| l == &lang.code);
                quick_row.push(choice_button(
                    &label,
                    format!("{CB_SUB_TOGGLE}{request_id}:{i}"),
                    selected,
                ));
            }
        }
        if !quick_row.is_empty() {
            rows.push(quick_row);
        }
        rows.push(vec![plain_button(
            &t("youtube.selection.subtitle_menu"),
            format!("{CB_SUB_MENU}{request_id}"),
        )]);
    }

    rows.push(vec![confirm_button(
        &t("youtube.selection.confirm"),
        format!("{CB_GO}{request_id}"),
    )]);
    rows.push(vec![primary_button(
        &t("youtube.selection.back_to_quality"),
        format!("{CB_BACK_TO_QUALITY_PREFIX}{request_id}"),
    )]);

    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

fn build_sub_menu_keyboard(
    req: &YoutubeRequest,
    request_id: u64,
    sel: &Selection,
    page: usize,
) -> InlineKeyboardMarkup {
    let total = req.subtitle_languages.len();
    let total_pages = total.div_ceil(SUB_PER_PAGE).max(1);
    let page = page.min(total_pages.saturating_sub(1));
    let start = page * SUB_PER_PAGE;
    let end = (start + SUB_PER_PAGE).min(total);

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    let header = tf(
        "youtube.selection.subtitle_menu_header",
        &[
            ("page", &(page + 1).to_string()),
            ("total", &total_pages.to_string()),
        ],
    );
    rows.push(vec![header_button(&header)]);

    let slice = &req.subtitle_languages[start..end];
    for (row_idx, chunk) in slice.chunks(SUB_PAGE_COLS).enumerate() {
        let row: Vec<InlineKeyboardButton> = chunk
            .iter()
            .enumerate()
            .map(|(col_idx, lang)| {
                let real_i = start + row_idx * SUB_PAGE_COLS + col_idx;
                let mut label = lang_name_fa(&lang.code);
                if lang.is_auto {
                    label.push_str(" 🤖");
                }
                let selected = sel.subtitle_langs.iter().any(|l| l == &lang.code);
                choice_button(
                    &label,
                    format!("{CB_SUB_TOGGLE}{request_id}:{real_i}"),
                    selected,
                )
            })
            .collect();
        rows.push(row);
    }

    let mut nav: Vec<InlineKeyboardButton> = Vec::new();
    if page > 0 {
        nav.push(plain_button(
            &t("youtube.selection.page_prev"),
            format!("{CB_SUB_PAGE}{request_id}:{}", page - 1),
        ));
    }
    nav.push(plain_button(
        &t("youtube.selection.subtitle_back"),
        format!("{CB_SUB_BACK}{request_id}"),
    ));
    if page + 1 < total_pages {
        nav.push(plain_button(
            &t("youtube.selection.page_next"),
            format!("{CB_SUB_PAGE}{request_id}:{}", page + 1),
        ));
    }
    rows.push(nav);

    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

fn header_button(text: &str) -> InlineKeyboardButton {
    button(text, CB_NOP.to_string(), None)
}

fn plain_button(text: &str, callback_data: String) -> InlineKeyboardButton {
    button(text, callback_data, None)
}

fn choice_button(text: &str, callback_data: String, selected: bool) -> InlineKeyboardButton {
    let style = if selected { Some(ButtonStyle::Success) } else { None };
    button(text, callback_data, style)
}

fn confirm_button(text: &str, callback_data: String) -> InlineKeyboardButton {
    button(text, callback_data, Some(ButtonStyle::Success))
}

fn primary_button(text: &str, callback_data: String) -> InlineKeyboardButton {
    button(text, callback_data, Some(ButtonStyle::Primary))
}

fn button(text: &str, callback_data: String, style: Option<ButtonStyle>) -> InlineKeyboardButton {
    InlineKeyboardButton {
        text: text.to_string(),
        icon_custom_emoji_id: None,
        callback_data: Some(callback_data),
        style,
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

async fn answer(api: &Bot, cq: &CallbackQuery, text_key: &str) {
    let mut params = AnswerCallbackQueryParams::builder()
        .callback_query_id(&cq.id)
        .build();
    if !text_key.is_empty() {
        params.text = Some(t(text_key));
    }
    let _ = api.answer_callback_query(&params).await;
}

fn quality_label(height: u32) -> String {
    let key = format!("youtube.quality.buttons.{height}");
    let label = t(&key);
    if label.starts_with('!') {
        format!("{height}p")
    } else {
        label
    }
}
