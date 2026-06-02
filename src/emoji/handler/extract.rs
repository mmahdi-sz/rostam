use frankenstein::types::{Message, MessageEntity, MessageEntityType};

use crate::emoji::PendingEmoji;

pub(super) fn extract_custom_emojis(message: &Message) -> Vec<PendingEmoji> {
    let mut out = Vec::new();
    let text = message.text.as_deref().unwrap_or("");
    if let Some(entities) = &message.entities {
        for entity in entities { push_custom_emoji(&mut out, text, entity); }
    }
    let caption = message.caption.as_deref().unwrap_or("");
    if let Some(entities) = &message.caption_entities {
        for entity in entities { push_custom_emoji(&mut out, caption, entity); }
    }
    out
}

fn push_custom_emoji(out: &mut Vec<PendingEmoji>, text: &str, entity: &MessageEntity) {
    if entity.type_field != MessageEntityType::CustomEmoji { return }
    let Some(id) = entity.custom_emoji_id.as_deref() else { return };
    let fallback = slice_utf16(text, entity.offset, entity.length);
    if fallback.is_empty() { return }
    out.push(PendingEmoji { custom_emoji_id: id.to_string(), fallback });
}

fn slice_utf16(text: &str, offset: u16, length: u16) -> String {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    let start = offset as usize;
    let end = (offset as usize + length as usize).min(utf16.len());
    if start >= utf16.len() { return String::new() }
    String::from_utf16_lossy(&utf16[start..end])
}
