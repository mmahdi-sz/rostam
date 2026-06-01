use std::sync::OnceLock;

use frankenstein::types::{MessageEntity, MessageEntityType};

static I18N: OnceLock<serde_json::Value> = OnceLock::new();

const I18N_JSON: &str = include_str!("../i18n.json");

fn root() -> &'static serde_json::Value {
    I18N.get_or_init(|| {
        serde_json::from_str(I18N_JSON).expect("i18n.json must be valid JSON")
    })
}

pub fn t(key: &str) -> String {
    let mut node = root();
    for segment in key.split('.') {
        match node.get(segment) {
            Some(next) => node = next,
            None => return format!("!{key}!"),
        }
    }
    node.as_str().map(|s| s.to_owned()).unwrap_or_else(|| format!("!{key}!"))
}

pub fn tf(key: &str, vars: &[(&str, &str)]) -> String {
    let mut template = t(key);
    for (name, value) in vars {
        let placeholder = format!("{{{name}}}");
        template = template.replace(&placeholder, value);
    }
    template
}

/// Scans `text` for known UI emoji and returns MessageEntity items
/// that map each one to its premium custom emoji ID from i18n icons.
/// Longer/variation-selector forms are tried before shorter ones.
pub fn entities_for_text(text: &str) -> Vec<MessageEntity> {
    // (emoji_str, icon_key) — longer variants must come before shorter ones with same prefix
    static EMOJI_MAP: &[(&str, &str)] = &[
        ("ℹ️", "info"),
        ("ℹ", "info"),
        ("⚠️", "warning"),
        ("⚠", "warning"),
        ("✂️", "edit"),
        ("✂", "edit"),
        ("✅", "confirm"),
        ("✏️", "test"),
        ("✏", "test"),
        ("❌", "cancel"),
        ("➕", "add"),
        ("➖", "remove"),
        ("➡️", "next"),
        ("➡", "next"),
        ("⬅️", "prev"),
        ("⬅", "prev"),
        ("🎨", "panel"),
        ("⭐️", "set_default"),
        ("⭐", "set_default"),
        ("💡", "hint"),
        ("📁", "packs"),
        ("📂", "pack_folder"),
        ("📄", "page"),
        ("📊", "stats"),
        ("📋", "list"),
        ("📥", "import"),
        ("📦", "export"),
        ("🔄", "replace_mode"),
        ("🗑️", "delete_pack"),
        ("🗑", "delete_pack"),
        ("🧹", "smart_merge"),
        ("🔙", "back"),
    ];

    let mut entities: Vec<MessageEntity> = Vec::new();
    let mut byte_pos: usize = 0;
    let mut utf16_offset: u32 = 0;

    while byte_pos < text.len() {
        let mut matched = false;
        for (emoji_str, icon_key) in EMOJI_MAP {
            if text[byte_pos..].starts_with(emoji_str) {
                let icon_id = t(&format!("emoji.panel.icons.{icon_key}"));
                if !icon_id.is_empty() {
                    let emoji_utf16_len: u32 =
                        emoji_str.chars().map(|c| c.len_utf16() as u32).sum();
                    entities.push(MessageEntity {
                        type_field: MessageEntityType::CustomEmoji,
                        offset: utf16_offset as u16,
                        length: emoji_utf16_len as u16,
                        url: None,
                        user: None,
                        language: None,
                        custom_emoji_id: Some(icon_id),
                        unix_time: None,
                        date_time_format: None,
                    });
                    utf16_offset += emoji_utf16_len;
                    byte_pos += emoji_str.len();
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            if let Some(c) = text[byte_pos..].chars().next() {
                utf16_offset += c.len_utf16() as u32;
                byte_pos += c.len_utf8();
            } else {
                break;
            }
        }
    }

    entities
}
