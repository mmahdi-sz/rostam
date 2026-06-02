use frankenstein::types::{MessageEntity, MessageEntityType};
use rand::seq::SliceRandom;

use super::types::{EmojiCache, EmojiEntry};

fn is_raw_id(key: &str) -> bool {
    key.len() >= 10 && key.chars().all(|c| c.is_ascii_digit())
}

fn escape_md_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#'
            | '+' | '-' | '=' | '|' | '{' | '}' | '.' | '!' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

fn escape_md_link_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if c == '\\' || c == ']' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[derive(Debug, Clone)]
pub enum LookupOutcome {
    CacheHit { custom_emoji_id: String, fallback: String, group_size: usize },
    RawId,
    NotFound,
    UnclosedBrace,
}

#[derive(Debug, Clone)]
pub struct RenderLookup {
    pub key: String,
    pub outcome: LookupOutcome,
}

impl EmojiCache {
    pub fn render_markdown_with_trace(
        &self,
        template: &str,
    ) -> (String, Vec<RenderLookup>) {
        let mut result = String::new();
        let mut lookups: Vec<RenderLookup> = Vec::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            result.push_str(&escape_md_text(&rest[..open]));
            let after = &rest[open + 1..];
            if let Some(close) = after.find('}') {
                let key = &after[..close];
                if let Some(entry) = self.pick(key) {
                    let group_size = self.group_size(key);
                    lookups.push(RenderLookup {
                        key: key.to_string(),
                        outcome: LookupOutcome::CacheHit {
                            custom_emoji_id: entry.custom_emoji_id.clone(),
                            fallback: entry.fallback.clone(),
                            group_size,
                        },
                    });
                    result.push_str(&format!(
                        "![{}](tg://emoji?id={})",
                        escape_md_link_text(&entry.fallback),
                        entry.custom_emoji_id,
                    ));
                } else if is_raw_id(key) {
                    lookups.push(RenderLookup {
                        key: key.to_string(),
                        outcome: LookupOutcome::RawId,
                    });
                    result.push_str(&format!("![⬛](tg://emoji?id={key})"));
                } else {
                    lookups.push(RenderLookup {
                        key: key.to_string(),
                        outcome: LookupOutcome::NotFound,
                    });
                    result.push_str(&escape_md_text(&format!("{{{key}}}")));
                }
                rest = &after[close + 1..];
            } else {
                lookups.push(RenderLookup {
                    key: String::new(),
                    outcome: LookupOutcome::UnclosedBrace,
                });
                result.push_str("\\{");
                rest = after;
            }
        }
        result.push_str(&escape_md_text(rest));
        (result, lookups)
    }

    pub fn render_plain_with_trace(
        &self,
        template: &str,
    ) -> (String, Vec<MessageEntity>, Vec<RenderLookup>) {
        let mut text = String::new();
        let mut entities: Vec<MessageEntity> = Vec::new();
        let mut lookups: Vec<RenderLookup> = Vec::new();
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
                    let group_size = self.group_size(key);
                    let len_utf16: u32 =
                        entry.fallback.chars().map(|c| c.len_utf16() as u32).sum();
                    lookups.push(RenderLookup {
                        key: key.to_string(),
                        outcome: LookupOutcome::CacheHit {
                            custom_emoji_id: entry.custom_emoji_id.clone(),
                            fallback: entry.fallback.clone(),
                            group_size,
                        },
                    });
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
                } else if is_raw_id(key) {
                    let fallback = "⬛";
                    let len_utf16: u32 = fallback.chars().map(|c| c.len_utf16() as u32).sum();
                    lookups.push(RenderLookup {
                        key: key.to_string(),
                        outcome: LookupOutcome::RawId,
                    });
                    entities.push(MessageEntity {
                        type_field: MessageEntityType::CustomEmoji,
                        offset: utf16_offset as u16,
                        length: len_utf16 as u16,
                        url: None, user: None, language: None,
                        custom_emoji_id: Some(key.to_string()),
                        unix_time: None, date_time_format: None,
                    });
                    text.push_str(fallback);
                    utf16_offset += len_utf16;
                } else {
                    lookups.push(RenderLookup {
                        key: key.to_string(),
                        outcome: LookupOutcome::NotFound,
                    });
                    let s = format!("{{{key}}}");
                    for c in s.chars() { utf16_offset += c.len_utf16() as u32; }
                    text.push_str(&s);
                }
                rest = &after[close + 1..];
            } else {
                lookups.push(RenderLookup {
                    key: String::new(),
                    outcome: LookupOutcome::UnclosedBrace,
                });
                text.push('{');
                utf16_offset += 1;
                rest = after;
            }
        }
        text.push_str(rest);
        (text, entities, lookups)
    }

    pub(super) fn pick(&self, key: &str) -> Option<EmojiEntry> {
        let entries = self.by_key.get(key)?;
        if entries.is_empty() { return None; }
        let mut rng = rand::thread_rng();
        entries.choose(&mut rng).cloned()
    }

    fn group_size(&self, key: &str) -> usize {
        self.by_key.get(key).map(|v| v.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    pub fn key_count(&self) -> usize {
        self.by_key.len()
    }

    pub fn entry_count(&self) -> usize {
        self.by_key.values().map(|v| v.len()).sum()
    }
}
