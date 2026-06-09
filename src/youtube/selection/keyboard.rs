use frankenstein::types::{InlineKeyboardButton, InlineKeyboardMarkup};

use crate::i18n::{t, tf};

use super::super::download::{Selection, SelectionView, SubtitleMode, YoutubeRequest, codecs_for_height, with_selection};
use super::super::lang_names::lang_name_fa;
use super::super::types::VideoCodec;
use super::buttons::{
    choice_button, confirm_button, header_button, icon_button, main_menu_button, plain_button, primary_button,
};
use super::constants::*;

const CODEC_DISPLAY_ORDER: &[VideoCodec] = &[
    VideoCodec::Av1,
    VideoCodec::Vp9,
    VideoCodec::H265,
    VideoCodec::H264,
];

pub fn build_keyboard(req: &YoutubeRequest, request_id: u64) -> InlineKeyboardMarkup {
    let selection = with_selection(req, |slot| slot.clone()).unwrap_or_else(|| Selection {
        height: 0,
        codec: VideoCodec::H264,
        audio_lang: None,
        subtitle_langs: Vec::new(),
        subtitle_mode: SubtitleMode::Embedded,
        view: SelectionView::Main,
    });
    match selection.view {
        SelectionView::Main => build_main_keyboard(req, request_id, &selection),
        SelectionView::SubMenu(page) => build_sub_menu_keyboard(req, request_id, &selection, page),
    }
}

fn build_main_keyboard(req: &YoutubeRequest, request_id: u64, sel: &Selection) -> InlineKeyboardMarkup {
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
                if lang.is_original { label.push_str(" ●"); }
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
        if !sel.subtitle_langs.is_empty() {
            let mode_selected = sel.subtitle_mode;
            rows.push(vec![
                choice_button(
                    &t("youtube.selection.subtitle_mode_file"),
                    format!("{CB_SUB_MODE}{request_id}:file"),
                    mode_selected == SubtitleMode::File,
                ),
                choice_button(
                    &t("youtube.selection.subtitle_mode_embedded"),
                    format!("{CB_SUB_MODE}{request_id}:embedded"),
                    mode_selected == SubtitleMode::Embedded,
                ),
            ]);
        }
    }

    rows.push(vec![confirm_button(&t("youtube.selection.confirm"), format!("{CB_GO}{request_id}"))]);
    rows.push(vec![primary_button(&t("youtube.selection.back_to_quality"), format!("{CB_BACK_TO_QUALITY_PREFIX}{request_id}"))]);
    rows.push(vec![main_menu_button()]);

    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

fn build_sub_menu_keyboard(req: &YoutubeRequest, request_id: u64, sel: &Selection, page: usize) -> InlineKeyboardMarkup {
    let total = req.subtitle_languages.len();
    let total_pages = total.div_ceil(SUB_PER_PAGE).max(1);
    let page = page.min(total_pages.saturating_sub(1));
    let start = page * SUB_PER_PAGE;
    let end = (start + SUB_PER_PAGE).min(total);

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    let header = tf(
        "youtube.selection.subtitle_menu_header",
        &[("page", &(page + 1).to_string()), ("total", &total_pages.to_string())],
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
                if lang.is_auto { label.push_str(" 🤖"); }
                let selected = sel.subtitle_langs.iter().any(|l| l == &lang.code);
                choice_button(&label, format!("{CB_SUB_TOGGLE}{request_id}:{real_i}"), selected)
            })
            .collect();
        rows.push(row);
    }

    let mut nav: Vec<InlineKeyboardButton> = Vec::new();
    if page > 0 {
        nav.push(icon_button(&t("youtube.selection.page_prev"), "emoji.panel.icons.prev", format!("{CB_SUB_PAGE}{request_id}:{}", page - 1), None));
    }
    if page + 1 < total_pages {
        nav.push(icon_button(&t("youtube.selection.page_next"), "emoji.panel.icons.next", format!("{CB_SUB_PAGE}{request_id}:{}", page + 1), None));
    }
    if !nav.is_empty() {
        rows.push(nav);
    }
    rows.push(vec![icon_button(&t("youtube.selection.subtitle_back"), "emoji.panel.icons.back", format!("{CB_SUB_BACK}{request_id}"), None)]);
    rows.push(vec![main_menu_button()]);

    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}
