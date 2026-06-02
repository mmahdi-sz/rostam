use frankenstein::types::{MessageEntity, MessageEntityType};

use super::lookup::t;
use super::emoji_map::EMOJI_MAP;

pub fn entities_for_text(text: &str) -> Vec<MessageEntity> {
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
