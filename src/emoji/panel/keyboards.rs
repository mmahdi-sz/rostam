use frankenstein::types::{
    ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton,
    ReplyKeyboardMarkup, ReplyKeyboardRemove,
};

use crate::i18n::{t, tf};

use super::buttons::{btn, btn_icon, btn_success, btn_danger};
use super::constants::*;
use super::super::store::{EmojiItem, EmojiPack};

pub fn main_panel_keyboard() -> InlineKeyboardMarkup {
    let add = btn_icon(&t("emoji.panel.add"), CB_ADD, "add");
    let test = btn_icon(&t("emoji.panel.test"), CB_TEST, "test");
    let list = btn_icon(&t("emoji.panel.list"), CB_LIST, "list");
    let del = btn_icon(&t("emoji.panel.delete_pack"), CB_DELETE_PACK_MENU, "delete_pack");
    let packs = btn_icon(&t("emoji.panel.packs"), CB_PACKS, "packs");
    let import = btn_icon(&t("emoji.panel.import"), CB_IMPORT, "import");
    let export = btn_icon(&t("emoji.panel.export"), CB_EXPORT, "export");
    let back = btn_icon(&t("emoji.panel.back"), CB_BACK, "back");
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![add], vec![test, list], vec![del, packs], vec![import, export], vec![back]])
        .build()
}

pub fn main_panel_text() -> String {
    t("emoji.panel.title")
}

pub fn packs_keyboard(packs: &[EmojiPack]) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    for pack in packs {
        let label = format!("{}  ({})", pack.name, pack.item_count);
        let icon = if pack.is_default { "set_default" } else { "pack_folder" };
        rows.push(vec![btn_icon(&label, &format!("{CB_PACK_OPEN_PREFIX}{}", pack.id), icon)]);
    }
    rows.push(vec![btn_icon(&t("emoji.panel.back"), CB_BACK, "back")]);
    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

pub fn pack_detail_keyboard(pack: &EmojiPack) -> InlineKeyboardMarkup {
    let set_alias = btn_icon(&t("emoji.panel.set_alias"), &format!("{CB_PACK_SET_ALIAS_PREFIX}{}", pack.id), "set_alias");
    let delete = btn_icon(&t("emoji.panel.delete_pack"), &format!("{CB_PACK_DELETE_PREFIX}{}", pack.id), "delete_pack");
    let mut rows = vec![vec![set_alias]];
    if !pack.is_default {
        rows.push(vec![btn_icon(&t("emoji.panel.set_default"), &format!("{CB_PACK_SET_DEFAULT_PREFIX}{}", pack.id), "set_default")]);
    }
    rows.push(vec![delete]);
    rows.push(vec![btn_icon(&t("emoji.panel.back_to_list"), CB_PACKS, "back")]);
    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

pub fn pack_detail_text(pack: &EmojiPack) -> String {
    tf(
        "emoji.pack_detail",
        &[
            ("id", &pack.id.to_string()),
            ("name", &pack.name),
            ("alias", pack.alias.as_deref().unwrap_or("-")),
            ("count", &pack.item_count.to_string()),
            ("default", if pack.is_default { "✅" } else { "-" }),
        ],
    )
}

pub fn list_page_keyboard(page: usize, total_pages: usize) -> InlineKeyboardMarkup {
    let mut nav: Vec<InlineKeyboardButton> = Vec::new();
    if page > 0 {
        nav.push(btn(&t("emoji.panel.prev"), &format!("{CB_LIST_PAGE_PREFIX}{}", page - 1)));
    }
    if page + 1 < total_pages {
        nav.push(btn(&t("emoji.panel.next"), &format!("{CB_LIST_PAGE_PREFIX}{}", page + 1)));
    }
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    if !nav.is_empty() { rows.push(nav); }
    rows.push(vec![btn_icon(&t("emoji.panel.back"), CB_BACK, "back")]);
    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

pub fn pack_choice_keyboard(packs: &[EmojiPack], page: usize, total_pages: usize) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    rows.push(vec![btn_success(&t("emoji.panel.show_pack_links"), CB_SHOW_PACK_LINKS)]);
    for pack in packs.iter().rev() {
        let icon = if pack.is_default { "set_default" } else { "pack_folder" };
        rows.push(vec![btn_icon(&pack.name, &format!("{CB_PICK_PACK_PREFIX}{}", pack.id), icon)]);
    }
    if total_pages > 1 {
        let mut nav: Vec<InlineKeyboardButton> = Vec::new();
        if page > 0 {
            nav.push(btn_icon(&t("emoji.panel.prev"), &format!("{CB_PENDING_PAGE_PREFIX}{}", page - 1), "prev"));
        }
        if page + 1 < total_pages {
            nav.push(btn_icon(&t("emoji.panel.next"), &format!("{CB_PENDING_PAGE_PREFIX}{}", page + 1), "next"));
        }
        if !nav.is_empty() { rows.push(nav); }
    }
    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

pub fn pack_links_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(&t("emoji.panel.back_to_pack_choice"), CB_BACK_TO_PACK_CHOICE, "back")]])
        .build()
}

pub fn cancel_reply_keyboard() -> ReplyKeyboardMarkup {
    let icon_id = t("emoji.panel.icons.cancel");
    let cancel_btn = KeyboardButton {
        text: t("emoji.cancel_button"),
        icon_custom_emoji_id: if icon_id.is_empty() { None } else { Some(icon_id) },
        request_users: None, request_chat: None, request_managed_bot: None,
        request_contact: None, request_location: None, request_poll: None,
        web_app: None, style: Some(ButtonStyle::Danger),
    };
    ReplyKeyboardMarkup::builder()
        .keyboard(vec![vec![cancel_btn]])
        .resize_keyboard(true)
        .one_time_keyboard(false)
        .build()
}

pub fn import_choice_keyboard(db_empty: bool) -> InlineKeyboardMarkup {
    let cancel = btn_icon(&t("emoji.cancel_button"), CB_CANCEL, "cancel");
    if db_empty {
        InlineKeyboardMarkup::builder()
            .inline_keyboard(vec![vec![btn(&t("emoji.import.btn_confirm"), CB_IMPORT_MERGE)], vec![cancel]])
            .build()
    } else {
        InlineKeyboardMarkup::builder()
            .inline_keyboard(vec![
                vec![btn(&t("emoji.import.btn_replace"), CB_IMPORT_REPLACE)],
                vec![btn(&t("emoji.import.btn_merge"), CB_IMPORT_MERGE)],
                vec![btn(&t("emoji.import.btn_smart"), CB_IMPORT_SMART)],
                vec![cancel],
            ])
            .build()
    }
}

pub fn remove_reply_keyboard() -> ReplyKeyboardRemove {
    ReplyKeyboardRemove::builder().remove_keyboard(true).build()
}

pub fn pack_delete_confirm_keyboard(pack_id: i32) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![
            btn_success(&t("emoji.panel.delete_confirm_yes"), &format!("{CB_PACK_DELETE_CONFIRM_PREFIX}{pack_id}")),
            btn_danger(&t("emoji.panel.delete_confirm_no"), &format!("{CB_PACK_OPEN_PREFIX}{pack_id}")),
        ]])
        .build()
}

fn escape_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '`' || ch == '\\' { out.push('\\'); }
        out.push(ch);
    }
    out
}

pub fn render_item_line_inner(item: &EmojiItem) -> String {
    let alias_part = match &item.alias {
        Some(a) if !a.is_empty() => format!(" \\| `{}`", escape_code(a)),
        _ => String::new(),
    };
    format!(
        "• ![{}](tg://emoji?id={}) {} \\= `{}` \\| `{}`{}\n",
        item.fallback, item.custom_emoji_id, item.fallback,
        item.id, escape_code(&item.smart_name), alias_part
    )
}
