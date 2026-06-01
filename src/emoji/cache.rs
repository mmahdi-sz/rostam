use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use frankenstein::types::{MessageEntity, MessageEntityType};
use rand::seq::SliceRandom;
use tokio::sync::RwLock;
use tokio_postgres::Client;

pub static CACHE: OnceLock<Arc<RwLock<EmojiCache>>> = OnceLock::new();

pub fn global() -> Option<Arc<RwLock<EmojiCache>>> {
    CACHE.get().cloned()
}

#[derive(Clone)]
pub struct EmojiEntry {
    pub custom_emoji_id: String,
    pub fallback: String,
}

#[derive(Clone, Default)]
pub struct EmojiCache {
    by_key: HashMap<String, Vec<EmojiEntry>>,
}

impl EmojiCache {
    /// Replace `{key}` with `![fallback](tg://emoji?id=ID)` for MarkdownV2.
    pub fn render_markdown(&self, template: &str) -> String {
        let mut result = String::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            result.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            if let Some(close) = after.find('}') {
                let key = &after[..close];
                if let Some(entry) = self.pick(key) {
                    result.push_str(&format!(
                        "![{}](tg://emoji?id={})",
                        entry.fallback, entry.custom_emoji_id
                    ));
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

    /// Replace `{key}` with the fallback char and return (text, entities).
    /// The caller merges these entities with any UI entities before sending.
    pub fn render_plain(&self, template: &str) -> (String, Vec<MessageEntity>) {
        let mut text = String::new();
        let mut entities: Vec<MessageEntity> = Vec::new();
        let mut utf16_offset: u32 = 0;
        let mut rest = template;

        while let Some(open) = rest.find('{') {
            let prefix = &rest[..open];
            for c in prefix.chars() {
                utf16_offset += c.len_utf16() as u32;
            }
            text.push_str(prefix);
            let after = &rest[open + 1..];
            if let Some(close) = after.find('}') {
                let key = &after[..close];
                if let Some(entry) = self.pick(key) {
                    let len_utf16: u32 =
                        entry.fallback.chars().map(|c| c.len_utf16() as u32).sum();
                    entities.push(MessageEntity {
                        type_field: MessageEntityType::CustomEmoji,
                        offset: utf16_offset as u16,
                        length: len_utf16 as u16,
                        url: None,
                        user: None,
                        language: None,
                        custom_emoji_id: Some(entry.custom_emoji_id.clone()),
                        unix_time: None,
                        date_time_format: None,
                    });
                    text.push_str(&entry.fallback);
                    utf16_offset += len_utf16;
                } else {
                    let s = format!("{{{key}}}");
                    for c in s.chars() {
                        utf16_offset += c.len_utf16() as u32;
                    }
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

    fn pick(&self, key: &str) -> Option<EmojiEntry> {
        let entries = self.by_key.get(key)?;
        if entries.is_empty() {
            return None;
        }
        let mut rng = rand::thread_rng();
        entries.choose(&mut rng).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

pub async fn load_from_db(client: &Client, admin_id: i64) -> EmojiCache {
    let rows = match client
        .query(
            "SELECT custom_emoji_id, fallback, smart_name, alias \
             FROM emoji_items WHERE owner_user_id = $1",
            &[&admin_id],
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("emoji cache load failed: {e}");
            return EmojiCache::default();
        }
    };

    let mut by_key: HashMap<String, Vec<EmojiEntry>> = HashMap::new();

    for row in rows {
        let custom_emoji_id: String = row.get(0);
        let fallback: String = row.get(1);
        let smart_name: String = row.get(2);
        let alias: Option<String> = row.get(3);

        let entry = EmojiEntry { custom_emoji_id, fallback };

        // Exact smart_name key (e.g. "fire1")
        by_key.entry(smart_name.clone()).or_default().push(entry.clone());

        // Prefix group (strip trailing digits, e.g. "fire")
        let prefix = smart_name.trim_end_matches(|c: char| c.is_ascii_digit());
        if !prefix.is_empty() && prefix != smart_name {
            by_key.entry(prefix.to_string()).or_default().push(entry.clone());
        }

        // Alias group
        if let Some(a) = alias.filter(|a| !a.is_empty()) {
            by_key.entry(a).or_default().push(entry);
        }
    }

    EmojiCache { by_key }
}
