use frankenstein::types::{MessageEntity, MessageEntityType};
use rand::seq::SliceRandom;

use super::types::{EmojiCache, EmojiEntry};

impl EmojiCache {
    pub fn render_markdown(&self, template: &str) -> String {
        let mut result = String::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            result.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            if let Some(close) = after.find('}') {
                let key = &after[..close];
                if let Some(entry) = self.pick(key) {
                    result.push_str(&format!("![{}](tg://emoji?id={})", entry.fallback, entry.custom_emoji_id));
                } else {
                    result.push('{');
                    result.push_str(key);
                    result.push('}');
                }
                rest = &after[close + 1..];
            } else {
                result.push('{');
                rest = after;
            }
        }
        result.push_str(rest);
        result
    }

    pub fn render_plain(&self, template: &str) -> (String, Vec<MessageEntity>) {
        let mut text = String::new();
        let mut entities: Vec<MessageEntity> = Vec::new();
        let mut utf16_offset: u32 = 0;
        let mut rest = template;

        while let Some(open) = rest.find('{') {
            let prefix = &rest[..open];
            for c in prefix.chars() { utf16_offset += c.len_utf16() as u32; }
            text.push_str(prefix);
            let after = &rest[open + 1..];
            if let Some(close) = after.find('}') {
                let key = &after[..close];
                if let Some(entry) = self.pick(key) {
                    let len_utf16: u32 = entry.fallback.chars().map(|c| c.len_utf16() as u32).sum();
                    entities.push(MessageEntity {
                        type_field: MessageEntityType::CustomEmoji,
                        offset: utf16_offset as u16,
                        length: len_utf16 as u16,
                        url: None, user: None, language: None,
                        custom_emoji_id: Some(entry.custom_emoji_id.clone()),
                        unix_time: None, date_time_format: None,
                    });
                    text.push_str(&entry.fallback);
                    utf16_offset += len_utf16;
                } else {
                    let s = format!("{{{key}}}");
                    for c in s.chars() { utf16_offset += c.len_utf16() as u32; }
                    text.push_str(&s);
                }
                rest = &after[close + 1..];
            } else {
                text.push('{');
                utf16_offset += 1;
                rest = after;
            }
        }
        text.push_str(rest);
        (text, entities)
    }

    pub(super) fn pick(&self, key: &str) -> Option<EmojiEntry> {
        let entries = self.by_key.get(key)?;
        if entries.is_empty() { return None; }
        let mut rng = rand::thread_rng();
        entries.choose(&mut rng).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}
