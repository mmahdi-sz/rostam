use crate::i18n::{apply_premium_to_md, tf, t};
use crate::youtube::escape_markdown_v2;

use super::constants::{LIST_PAGE_SIZE, PENDING_PAGE_SIZE, pending_total_pages};
use super::super::flow::PendingEmoji;
use super::super::store::{EmojiItem, EmojiPack};
use super::keyboards::render_item_line_inner;

pub fn format_pending_emojis(
    items: &[PendingEmoji],
    duplicates: &[PendingEmoji],
    page: usize,
) -> String {
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
            global_num, item.fallback, item.fallback, item.custom_emoji_id, item.custom_emoji_id
        ));
    }
    lines.push(String::new());
    lines.push(emd(&tf("emoji.pending.ready", &[("count", &items.len().to_string())])));
    if total_pages > 1 {
        lines.push(emd(&tf("emoji.pending.page_info", &[("page", &(page + 1).to_string()), ("pages", &total_pages.to_string())])));
    }
    {
        let choose_pack = t("emoji.pending.choose_pack");
        let formatted = choose_pack.lines().enumerate().map(|(i, line)| {
            if i < 2 {
                format!("*{}*", escape_markdown_v2(line))
            } else {
                escape_markdown_v2(line)
            }
        }).collect::<Vec<_>>().join("\n");
        lines.push(apply_premium_to_md(&formatted));
    }
    if !duplicates.is_empty() {
        let mut rendered = String::new();
        for d in duplicates {
            rendered.push_str(&format!("![{}](tg://emoji?id={})", d.fallback, d.custom_emoji_id));
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
    let emd = |s: &str| apply_premium_to_md(&escape_markdown_v2(s));
    let total_items: usize = packs_with_items.iter().map(|(_, i)| i.len()).sum();
    let total_pages = if total_items == 0 { 1 } else { (total_items + LIST_PAGE_SIZE - 1) / LIST_PAGE_SIZE };
    let page = page.min(total_pages.saturating_sub(1));
    let start = page * LIST_PAGE_SIZE;
    let end = (start + LIST_PAGE_SIZE).min(total_items);

    let mut out = String::new();
    out.push_str(&emd(&t("emoji.list_header")));
    out.push('\n');

    let mut seen = 0_usize;
    for (pack, items) in packs_with_items {
        if items.is_empty() { continue; }
        let pack_start = seen;
        let pack_end = seen + items.len();
        if pack_end <= start || pack_start >= end { seen = pack_end; continue; }
        out.push_str(&emd(&tf("emoji.list.pack_header", &[("name", &pack.name)])));
        out.push('\n');
        for (local_idx, item) in items.iter().enumerate() {
            let global = pack_start + local_idx;
            if global < start || global >= end { continue; }
            out.push_str(&render_item_line_inner(item));
        }
        seen = pack_end;
    }
    out.push_str(&emd(&tf(
        "emoji.list_page_footer",
        &[("page", &(page + 1).to_string()), ("pages", &total_pages.to_string()), ("total", &total_items.to_string())],
    )));
    (out, page, total_pages)
}

pub fn render_pack_list_entry(pack: &EmojiPack, items: &[EmojiItem]) -> String {
    let mut out = String::new();
    out.push_str(&escape_markdown_v2(&tf("emoji.list.pack_header", &[("name", &pack.name)])));
    out.push('\n');
    for item in items {
        out.push_str(&render_item_line_inner(item));
    }
    out
}
