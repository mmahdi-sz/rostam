use frankenstein::types::{
    ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton, ReplyKeyboardMarkup,
    ReplyKeyboardRemove,
};

use crate::i18n::{t, tf};

use super::store::{EmojiItem, EmojiPack};

pub const CB_ADD: &str = "emoji:add";
pub const CB_TEST: &str = "emoji:test";
pub const CB_LIST: &str = "emoji:list";
pub const CB_DELETE_PACK_MENU: &str = "emoji:delpack";
pub const CB_PACKS: &str = "emoji:packs";
pub const CB_IMPORT: &str = "emoji:import";
pub const CB_EXPORT: &str = "emoji:export";
pub const CB_BACK: &str = "emoji:back";
pub const CB_CANCEL: &str = "emoji:cancel";
pub const CB_PACK_OPEN_PREFIX: &str = "emoji:pack:";
pub const CB_PACK_SET_DEFAULT_PREFIX: &str = "emoji:setdef:";
pub const CB_PACK_SET_ALIAS_PREFIX: &str = "emoji:setalias:";
pub const CB_PACK_DELETE_PREFIX: &str = "emoji:packdel:";
pub const CB_LIST_PAGE_PREFIX: &str = "emoji:listpg:";
pub const LIST_PAGE_SIZE: usize = 15;

pub fn main_panel_keyboard() -> InlineKeyboardMarkup {
    let add = btn(&t("emoji.panel.add"), CB_ADD);
    let test = btn(&t("emoji.panel.test"), CB_TEST);
    let list = btn(&t("emoji.panel.list"), CB_LIST);
    let del = btn(&t("emoji.panel.delete_pack"), CB_DELETE_PACK_MENU);
    let packs = btn(&t("emoji.panel.packs"), CB_PACKS);
    let import = btn(&t("emoji.panel.import"), CB_IMPORT);
    let export = btn(&t("emoji.panel.export"), CB_EXPORT);
    let back = btn(&t("emoji.panel.back"), CB_BACK);

    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![add],
            vec![test, list],
            vec![del, packs],
            vec![import, export],
            vec![back],
        ])
        .build()
}

pub fn main_panel_text() -> String {
    t("emoji.panel.title")
}

pub fn packs_keyboard(packs: &[EmojiPack]) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    for pack in packs {
        let marker = if pack.is_default { " ⭐" } else { "" };
        let label = format!("{}{}  ({})", pack.name, marker, pack.item_count);
        rows.push(vec![btn(&label, &format!("{CB_PACK_OPEN_PREFIX}{}", pack.id))]);
    }
    rows.push(vec![btn(&t("emoji.panel.back"), CB_BACK)]);
    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

pub fn pack_detail_keyboard(pack: &EmojiPack) -> InlineKeyboardMarkup {
    let set_alias = btn(
        &t("emoji.panel.set_alias"),
        &format!("{CB_PACK_SET_ALIAS_PREFIX}{}", pack.id),
    );
    let delete = btn(
        &t("emoji.panel.delete_pack"),
        &format!("{CB_PACK_DELETE_PREFIX}{}", pack.id),
    );
    let mut rows = vec![vec![set_alias]];
    if !pack.is_default {
        rows.push(vec![btn(
            &t("emoji.panel.set_default"),
            &format!("{CB_PACK_SET_DEFAULT_PREFIX}{}", pack.id),
        )]);
    }
    rows.push(vec![delete]);
    rows.push(vec![btn(&t("emoji.panel.back_to_list"), CB_PACKS)]);
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
            (
                "default",
                if pack.is_default { "✅" } else { "-" },
            ),
        ],
    )
}

pub fn format_pending_emojis(
    items: &[super::flow::PendingEmoji],
    duplicates: &[super::flow::PendingEmoji],
) -> String {
    use crate::youtube::escape_markdown_v2;
    let mut lines: Vec<String> = Vec::new();
    lines.push(escape_markdown_v2(&t("emoji.pending.title")));
    for (idx, item) in items.iter().enumerate() {
        lines.push(format!(
            "`{}.` ![{}](tg://emoji?id={}) \\= `{}`",
            idx + 1,
            item.fallback,
            item.custom_emoji_id,
            item.custom_emoji_id
        ));
    }
    lines.push(String::new());
    lines.push(escape_markdown_v2(&tf(
        "emoji.pending.ready",
        &[("count", &items.len().to_string())],
    )));
    lines.push(escape_markdown_v2(&t("emoji.pending.choose_pack")));
    if !duplicates.is_empty() {
        let mut rendered = String::new();
        for d in duplicates {
            rendered.push_str(&format!(
                "![{}](tg://emoji?id={})",
                d.fallback, d.custom_emoji_id
            ));
        }
        let prefix = escape_markdown_v2("ℹ️ ایموجی‌های ");
        let suffix = escape_markdown_v2(" تکراری بودند و در لیست نیومدند.");
        lines.push(String::new());
        lines.push(format!("{prefix}{rendered}{suffix}"));
    }
    lines.join("\n")
}

pub fn build_list_page(
    packs_with_items: &[(EmojiPack, Vec<EmojiItem>)],
    page: usize,
) -> (String, usize, usize) {
    use crate::youtube::escape_markdown_v2;
    let total_items: usize = packs_with_items.iter().map(|(_, i)| i.len()).sum();
    let total_pages = if total_items == 0 {
        1
    } else {
        (total_items + LIST_PAGE_SIZE - 1) / LIST_PAGE_SIZE
    };
    let page = page.min(total_pages.saturating_sub(1));
    let start = page * LIST_PAGE_SIZE;
    let end = (start + LIST_PAGE_SIZE).min(total_items);

    let mut out = String::new();
    out.push_str(&escape_markdown_v2(&t("emoji.list_header")));
    out.push('\n');

    let mut seen = 0_usize;
    for (pack, items) in packs_with_items {
        if items.is_empty() {
            continue;
        }
        let pack_start = seen;
        let pack_end = seen + items.len();
        if pack_end <= start || pack_start >= end {
            seen = pack_end;
            continue;
        }
        out.push_str(&escape_markdown_v2(&tf(
            "emoji.list.pack_header",
            &[("name", &pack.name)],
        )));
        out.push('\n');
        for (local_idx, item) in items.iter().enumerate() {
            let global = pack_start + local_idx;
            if global < start || global >= end {
                continue;
            }
            out.push_str(&render_item_line(item));
        }
        seen = pack_end;
    }
    out.push_str(&escape_markdown_v2(&tf(
        "emoji.list_page_footer",
        &[
            ("page", &(page + 1).to_string()),
            ("pages", &total_pages.to_string()),
            ("total", &total_items.to_string()),
        ],
    )));
    (out, page, total_pages)
}

pub fn list_page_keyboard(page: usize, total_pages: usize) -> InlineKeyboardMarkup {
    let mut nav: Vec<InlineKeyboardButton> = Vec::new();
    if page > 0 {
        nav.push(btn(
            &t("emoji.panel.prev"),
            &format!("{CB_LIST_PAGE_PREFIX}{}", page - 1),
        ));
    }
    if page + 1 < total_pages {
        nav.push(btn(
            &t("emoji.panel.next"),
            &format!("{CB_LIST_PAGE_PREFIX}{}", page + 1),
        ));
    }
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    if !nav.is_empty() {
        rows.push(nav);
    }
    rows.push(vec![btn(&t("emoji.panel.back"), CB_BACK)]);
    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

fn render_item_line(item: &EmojiItem) -> String {
    let alias_part = match &item.alias {
        Some(a) if !a.is_empty() => format!(" \\| `{}`", escape_code(a)),
        _ => String::new(),
    };
    format!(
        "• ![{}](tg://emoji?id={}) \\= `{}` \\| `{}`{}\n",
        item.fallback,
        item.custom_emoji_id,
        item.id,
        escape_code(&item.smart_name),
        alias_part
    )
}

pub fn render_pack_list_entry(pack: &EmojiPack, items: &[EmojiItem]) -> String {
    use crate::youtube::escape_markdown_v2;
    let mut out = String::new();
    out.push_str(&escape_markdown_v2(&tf(
        "emoji.list.pack_header",
        &[("name", &pack.name)],
    )));
    out.push('\n');
    for item in items {
        let alias_part = match &item.alias {
            Some(a) if !a.is_empty() => format!(" \\| `{}`", escape_code(a)),
            _ => String::new(),
        };
        out.push_str(&format!(
            "• ![{}](tg://emoji?id={}) \\= `{}` \\| `{}`{}\n",
            item.fallback,
            item.custom_emoji_id,
            item.id,
            escape_code(&item.smart_name),
            alias_part
        ));
    }
    out
}

fn escape_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '`' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn btn(text: &str, callback_data: &str) -> InlineKeyboardButton {
    InlineKeyboardButton::builder()
        .text(text)
        .callback_data(callback_data)
        .style(ButtonStyle::Primary)
        .build()
}

pub fn cancel_reply_keyboard() -> ReplyKeyboardMarkup {
    let cancel_btn = KeyboardButton::builder()
        .text(t("emoji.cancel_button"))
        .build();
    ReplyKeyboardMarkup::builder()
        .keyboard(vec![vec![cancel_btn]])
        .resize_keyboard(true)
        .one_time_keyboard(false)
        .build()
}

pub fn remove_reply_keyboard() -> ReplyKeyboardRemove {
    ReplyKeyboardRemove::builder()
        .remove_keyboard(true)
        .build()
}
