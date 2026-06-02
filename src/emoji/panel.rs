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
pub const CB_PICK_PACK_PREFIX: &str = "emoji:pickpack:";
pub const CB_IMPORT_REPLACE: &str = "emoji:import:replace";
pub const CB_IMPORT_MERGE: &str = "emoji:import:merge";
pub const CB_IMPORT_SMART: &str = "emoji:import:smart";
pub const CB_SHOW_PACK_LINKS: &str = "emoji:packlinks";
pub const CB_BACK_TO_PACK_CHOICE: &str = "emoji:backpick";
pub const CB_PENDING_PAGE_PREFIX: &str = "emoji:pendpg:";
pub const LIST_PAGE_SIZE: usize = 15;
pub const PENDING_PAGE_SIZE: usize = 30;

pub fn pending_total_pages(count: usize) -> usize {
    if count == 0 { 1 } else { (count + PENDING_PAGE_SIZE - 1) / PENDING_PAGE_SIZE }
}

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
        let label = format!("{}  ({})", pack.name, pack.item_count);
        let icon = if pack.is_default { "set_default" } else { "pack_folder" };
        rows.push(vec![btn_icon(&label, &format!("{CB_PACK_OPEN_PREFIX}{}", pack.id), icon)]);
    }
    rows.push(vec![btn_icon(&t("emoji.panel.back"), CB_BACK, "back")]);
    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

pub fn pack_detail_keyboard(pack: &EmojiPack) -> InlineKeyboardMarkup {
    let set_alias = btn_icon(
        &t("emoji.panel.set_alias"),
        &format!("{CB_PACK_SET_ALIAS_PREFIX}{}", pack.id),
        "set_alias",
    );
    let delete = btn_icon(
        &t("emoji.panel.delete_pack"),
        &format!("{CB_PACK_DELETE_PREFIX}{}", pack.id),
        "delete_pack",
    );
    let mut rows = vec![vec![set_alias]];
    if !pack.is_default {
        rows.push(vec![btn_icon(
            &t("emoji.panel.set_default"),
            &format!("{CB_PACK_SET_DEFAULT_PREFIX}{}", pack.id),
            "set_default",
        )]);
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
    page: usize,
) -> String {
    use crate::i18n::apply_premium_to_md;
    use crate::youtube::escape_markdown_v2;
    let emd = |s: &str| apply_premium_to_md(&escape_markdown_v2(s));
    let total_pages = pending_total_pages(items.len());
    let page = page.min(total_pages.saturating_sub(1));
    let start = page * PENDING_PAGE_SIZE;
    let end = (start + PENDING_PAGE_SIZE).min(items.len());

    let mut lines: Vec<String> = Vec::new();
    lines.push(emd(&t("emoji.pending.title")));
    for (local_idx, item) in items[start..end].iter().enumerate() {
        let global_num = start + local_idx + 1;
        lines.push(format!(
            "`{}.` {} ![{}](tg://emoji?id={}) \\= `{}`",
            global_num,
            item.fallback,
            item.fallback,
            item.custom_emoji_id,
            item.custom_emoji_id
        ));
    }
    lines.push(String::new());
    lines.push(emd(&tf(
        "emoji.pending.ready",
        &[("count", &items.len().to_string())],
    )));
    if total_pages > 1 {
        lines.push(emd(&tf(
            "emoji.pending.page_info",
            &[("page", &(page + 1).to_string()), ("pages", &total_pages.to_string())],
        )));
    }
    lines.push(emd(&t("emoji.pending.choose_pack")));
    if !duplicates.is_empty() {
        let mut rendered = String::new();
        for d in duplicates {
            rendered.push_str(&format!(
                "![{}](tg://emoji?id={})",
                d.fallback, d.custom_emoji_id
            ));
        }
        let prefix = emd("ℹ️ ایموجی‌های ");
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
    use crate::i18n::apply_premium_to_md;
    use crate::youtube::escape_markdown_v2;
    let emd = |s: &str| apply_premium_to_md(&escape_markdown_v2(s));
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
    out.push_str(&emd(&t("emoji.list_header")));
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
        out.push_str(&emd(&tf(
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
    out.push_str(&emd(&tf(
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
    rows.push(vec![btn_icon(&t("emoji.panel.back"), CB_BACK, "back")]);
    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

fn render_item_line(item: &EmojiItem) -> String {
    let alias_part = match &item.alias {
        Some(a) if !a.is_empty() => format!(" \\| `{}`", escape_code(a)),
        _ => String::new(),
    };
    format!(
        "• ![{}](tg://emoji?id={}) {} \\= `{}` \\| `{}`{}\n",
        item.fallback,
        item.custom_emoji_id,
        item.fallback,
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
            "• ![{}](tg://emoji?id={}) {} \\= `{}` \\| `{}`{}\n",
            item.fallback,
            item.custom_emoji_id,
            item.fallback,
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

pub fn btn(text: &str, callback_data: &str) -> InlineKeyboardButton {
    btn_icon(text, callback_data, "")
}

pub fn btn_success(text: &str, callback_data: &str) -> InlineKeyboardButton {
    InlineKeyboardButton {
        text: text.to_string(),
        icon_custom_emoji_id: None,
        callback_data: Some(callback_data.to_string()),
        style: Some(ButtonStyle::Success),
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

fn btn_icon(text: &str, callback_data: &str, icon_key: &str) -> InlineKeyboardButton {
    let icon_id = if icon_key.is_empty() {
        None
    } else {
        let id = t(&format!("emoji.panel.icons.{icon_key}"));
        // t() returns "!key!" when the key is missing — reject those
        if id.is_empty() || id.starts_with('!') { None } else { Some(id) }
    };

    InlineKeyboardButton {
        text: text.to_string(),
        icon_custom_emoji_id: icon_id,
        callback_data: Some(callback_data.to_string()),
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
            nav.push(btn_icon(
                &t("emoji.panel.prev"),
                &format!("{CB_PENDING_PAGE_PREFIX}{}", page - 1),
                "prev",
            ));
        }
        if page + 1 < total_pages {
            nav.push(btn_icon(
                &t("emoji.panel.next"),
                &format!("{CB_PENDING_PAGE_PREFIX}{}", page + 1),
                "next",
            ));
        }
        if !nav.is_empty() {
            rows.push(nav);
        }
    }
    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

pub fn pack_links_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(
            &t("emoji.panel.back_to_pack_choice"),
            CB_BACK_TO_PACK_CHOICE,
            "back",
        )]])
        .build()
}

pub fn cancel_reply_keyboard() -> ReplyKeyboardMarkup {
    let icon_id = t("emoji.panel.icons.cancel");
    let cancel_btn = KeyboardButton {
        text: t("emoji.cancel_button"),
        icon_custom_emoji_id: if icon_id.is_empty() { None } else { Some(icon_id) },
        request_users: None,
        request_chat: None,
        request_managed_bot: None,
        request_contact: None,
        request_location: None,
        request_poll: None,
        web_app: None,
        style: Some(ButtonStyle::Danger),
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
            .inline_keyboard(vec![
                vec![btn(&t("emoji.import.btn_confirm"), CB_IMPORT_MERGE)],
                vec![cancel],
            ])
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
    ReplyKeyboardRemove::builder()
        .remove_keyboard(true)
        .build()
}
