use std::collections::HashMap;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::GetCustomEmojiStickersParams,
};

use crate::i18n::t;
use crate::youtube::escape_markdown_v2;
use crate::emoji::PendingEmoji;

pub(super) async fn build_pack_links_text(
    api: &Bot, collected: &[PendingEmoji], ids: &[String],
) -> String {
    if ids.is_empty() { return escape_markdown_v2(&t("emoji.pack_links_none")); }

    let stickers = match api.get_custom_emoji_stickers(
        &GetCustomEmojiStickersParams::builder().custom_emoji_ids(ids.to_vec()).build(),
    ).await {
        Ok(r) => r.result,
        Err(e) => { eprintln!("get_custom_emoji_stickers failed: {e}"); return escape_markdown_v2(&t("emoji.pack_links_none")); }
    };

    let mut id_to_set: HashMap<String, String> = HashMap::new();
    for sticker in &stickers {
        if let (Some(eid), Some(sn)) = (&sticker.custom_emoji_id, &sticker.set_name) {
            id_to_set.insert(eid.clone(), sn.clone());
        }
    }

    let mut set_order: Vec<String> = Vec::new();
    let mut set_to_entries: HashMap<String, Vec<&PendingEmoji>> = HashMap::new();
    for emoji in collected {
        let key = id_to_set.get(&emoji.custom_emoji_id).cloned().unwrap_or_else(|| "unknown".to_string());
        if !set_to_entries.contains_key(&key) { set_order.push(key.clone()); }
        set_to_entries.entry(key).or_default().push(emoji);
    }

    let mut lines = Vec::new();
    for set_name in &set_order {
        let entries = &set_to_entries[set_name];
        let emoji_line: String = entries.iter()
            .map(|e| format!("![{}](tg://emoji?id={})", e.fallback, e.custom_emoji_id))
            .collect::<Vec<_>>().join("");
        if set_name == "unknown" {
            lines.push(format!("{}{}", emoji_line, escape_markdown_v2(":\n(پک ناشناخته)")));
        } else {
            lines.push(format!(
                "{}{}\n{}",
                emoji_line,
                escape_markdown_v2(":"),
                escape_markdown_v2(&format!("https://t.me/addemoji/{}", set_name))
            ));
        }
    }

    if lines.is_empty() { escape_markdown_v2(&t("emoji.pack_links_none")) } else { lines.join("\n\n") }
}
