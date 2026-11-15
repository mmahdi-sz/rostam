//! Link and entity extraction helpers.
//!
//! Telegram represents entity offsets and lengths in UTF-16 code units.
//! This module safely decodes UTF-16 ranges without slicing Rust UTF-8 &str directly,
//! and provides unified link extraction from Message text, caption, and entities.

use frankenstein::types::{Message, MessageEntity, MessageEntityType};

/// Safely extracts a UTF-16 slice from a UTF-8 string using UTF-16 code unit offsets.
///
/// Telegram entities express `offset` and `length` in UTF-16 code units (surrogate pairs count as 2).
/// Slicing Rust's UTF-8 `&str` directly with byte offsets will panic on multibyte characters
/// (Persian, Arabic, emojis, etc.).
pub fn slice_utf16(text: &str, offset: usize, length: usize) -> String {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    if offset >= utf16.len() {
        return String::new();
    }
    let end = offset.saturating_add(length).min(utf16.len());
    String::from_utf16_lossy(&utf16[offset..end])
}

/// Extracts URLs from text and its associated entities.
///
/// Handles:
/// - `MessageEntityType::TextLink`: extracts `entity.url`.
/// - `MessageEntityType::Url`: extracts the slice from `text` using `offset` and `length` via `slice_utf16`.
pub fn extract_entity_links(text: &str, entities: &[MessageEntity]) -> Vec<String> {
    let mut links = Vec::new();
    for entity in entities {
        match entity.type_field {
            MessageEntityType::TextLink => {
                if let Some(ref url) = entity.url {
                    let trimmed = url.trim();
                    if !trimmed.is_empty() && !links.iter().any(|l| l == trimmed) {
                        links.push(trimmed.to_string());
                    }
                }
            }
            MessageEntityType::Url => {
                let extracted = slice_utf16(text, entity.offset as usize, entity.length as usize);
                let trimmed = extracted.trim();
                if !trimmed.is_empty() && !links.iter().any(|l| l == trimmed) {
                    links.push(trimmed.to_string());
                }
            }
            _ => {}
        }
    }
    links
}

/// Extracts plain URLs from text by scanning whitespace-separated tokens.
pub fn extract_plain_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for token in text.split(|c: char| c.is_whitespace()) {
        let token = token.trim_matches(|c: char| {
            matches!(
                c,
                '<' | '>' | '"' | '\'' | ',' | ';' | '!' | '?' | ')' | '(' | '[' | ']'
            )
        });
        if (token.starts_with("http://") || token.starts_with("https://"))
            && reqwest::Url::parse(token).is_ok()
            && !urls.iter().any(|u| u == token)
        {
            urls.push(token.to_string());
        }
    }
    urls
}

/// Extracts all candidate links from a `Message`, checking:
/// 1. `message.text` + `message.entities`
/// 2. `message.caption` + `message.caption_entities`
/// 3. Plain URLs from `message.text`
/// 4. Plain URLs from `message.caption`
///
/// Returns links deduplicated in order of appearance.
pub fn extract_message_links(message: &Message) -> Vec<String> {
    let mut links = Vec::new();

    let mut push_unique = |item: String| {
        let trimmed = item.trim().to_string();
        if !trimmed.is_empty() && !links.iter().any(|l| l == &trimmed) {
            links.push(trimmed);
        }
    };

    // 1. Text entity links
    if let Some(text) = message.text.as_deref() {
        if let Some(entities) = &message.entities {
            for link in extract_entity_links(text, entities) {
                push_unique(link);
            }
        }
    }

    // 2. Caption entity links
    if let Some(caption) = message.caption.as_deref() {
        if let Some(entities) = &message.caption_entities {
            for link in extract_entity_links(caption, entities) {
                push_unique(link);
            }
        }
    }

    // 3. Plain URLs from text
    if let Some(text) = message.text.as_deref() {
        for link in extract_plain_urls(text) {
            push_unique(link);
        }
    }

    // 4. Plain URLs from caption
    if let Some(caption) = message.caption.as_deref() {
        for link in extract_plain_urls(caption) {
            push_unique(link);
        }
    }

    links
}

/// Extracts the most appropriate download URL for Surge DL from a `Message`.
///
/// Looks for:
/// 1. A candidate URL matching `is_direct_link`.
/// 2. A candidate URL matching `detect_social_platform`.
/// 3. The first extracted URL (if any).
/// 4. Falls back to raw trimmed text/caption if non-empty.
pub fn extract_surge_url(message: &Message) -> Option<String> {
    let candidate_links = extract_message_links(message);

    // Prefer direct link
    for link in &candidate_links {
        if crate::surge_dl::is_direct_link(link) {
            return Some(link.clone());
        }
    }

    // Then social platform link
    for link in &candidate_links {
        if crate::surge_dl::detect_social_platform(link).is_some() {
            return Some(link.clone());
        }
    }

    // Then any candidate link
    if let Some(first) = candidate_links.into_iter().next() {
        return Some(first);
    }

    // Fallback to raw text or caption
    message
        .text
        .as_deref()
        .or(message.caption.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_message(json: &str) -> Message {
        serde_json::from_str::<Message>(json).expect("valid test Message JSON")
    }

    #[test]
    fn test_slice_utf16_ascii() {
        let text = "Hello, world!";
        assert_eq!(slice_utf16(text, 7, 5), "world");
        assert_eq!(slice_utf16(text, 0, 5), "Hello");
    }

    #[test]
    fn test_slice_utf16_persian() {
        // Persian text: characters are multibyte in UTF-8 (2 bytes each), but single code unit in UTF-16.
        let text = "دانلود فایل ویدیو";
        // "دانلود" has 6 code units.
        assert_eq!(slice_utf16(text, 0, 6), "دانلود");
        // "فایل" starts at offset 7, length 4.
        assert_eq!(slice_utf16(text, 7, 4), "فایل");
    }

    #[test]
    fn test_slice_utf16_emoji() {
        // "🌹" (U+1F339) is 4 bytes in UTF-8, and 2 code units in UTF-16.
        let text = "سلام 🌹 دنیا";
        // "سلام " = 5 code units.
        // "🌹" = 2 code units (offsets 5..7).
        // " دنیا" = 5 code units (offsets 7..12).
        assert_eq!(slice_utf16(text, 5, 2), "🌹");
        assert_eq!(slice_utf16(text, 8, 4), "دنیا");
    }

    #[test]
    fn test_slice_utf16_out_of_bounds_and_overflow() {
        let text = "سلام";
        assert_eq!(slice_utf16(text, 10, 5), "");
        assert_eq!(slice_utf16(text, 0, 100), "سلام");
        assert_eq!(slice_utf16(text, usize::MAX - 2, 5), "");
        assert_eq!(slice_utf16("", 0, 5), "");
        assert_eq!(slice_utf16(text, 2, 0), "");
    }

    #[test]
    fn test_extract_entity_links_text_link() {
        let entities = vec![MessageEntity {
            type_field: MessageEntityType::TextLink,
            offset: 0,
            length: 4,
            url: Some("https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT".to_string()),
            user: None,
            language: None,
            custom_emoji_id: None,
            unix_time: None,
            date_time_format: None,
        }];
        let links = extract_entity_links("اینجا", &entities);
        assert_eq!(
            links,
            vec!["https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT"]
        );
    }

    #[test]
    fn test_extract_entity_links_url_entity_persian_prefix() {
        let text = "سلام 🎵 لینک: https://youtu.be/dQw4w9WgXcQ عالی";
        // Calculate UTF-16 offset of URL:
        let prefix = "سلام 🎵 لینک: ";
        let offset = prefix.encode_utf16().count() as u16;
        let url_str = "https://youtu.be/dQw4w9WgXcQ";
        let length = url_str.encode_utf16().count() as u16;

        let entities = vec![MessageEntity {
            type_field: MessageEntityType::Url,
            offset,
            length,
            url: None,
            user: None,
            language: None,
            custom_emoji_id: None,
            unix_time: None,
            date_time_format: None,
        }];

        let links = extract_entity_links(text, &entities);
        assert_eq!(links, vec!["https://youtu.be/dQw4w9WgXcQ"]);
    }

    #[test]
    fn test_extract_message_links_caption_and_text() {
        let msg = make_test_message(
            r#"{
                "message_id": 1,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "caption": "برای دانلود فایل کلیک کنید",
                "caption_entities": [
                    {
                        "type": "text_link",
                        "offset": 12,
                        "length": 4,
                        "url": "https://example.com/movie.mp4"
                    }
                ]
            }"#,
        );

        let links = extract_message_links(&msg);
        assert_eq!(links, vec!["https://example.com/movie.mp4"]);
    }

    #[test]
    fn test_extract_message_links_plain_urls() {
        let msg = make_test_message(
            r#"{
                "message_id": 1,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "text": "دانلود از https://example.com/file.zip یا https://youtu.be/abc ممنون"
            }"#,
        );

        let links = extract_message_links(&msg);
        assert_eq!(
            links,
            vec!["https://example.com/file.zip", "https://youtu.be/abc"]
        );
    }

    #[test]
    fn test_extract_surge_url_priority() {
        // Direct link takes precedence over plain text or social
        let msg = make_test_message(
            r#"{
                "message_id": 1,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "text": "لینک دانلود: [دانلود مستقیم](https://example.com/archive.zip)",
                "entities": [
                    {
                        "type": "text_link",
                        "offset": 12,
                        "length": 13,
                        "url": "https://example.com/archive.zip"
                    }
                ]
            }"#,
        );

        assert_eq!(
            extract_surge_url(&msg),
            Some("https://example.com/archive.zip".to_string())
        );

        // Fallback to raw text when no valid URL is present
        let msg_raw = make_test_message(
            r#"{
                "message_id": 2,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "text": "   سلام متن نامعتبر   "
            }"#,
        );
        assert_eq!(
            extract_surge_url(&msg_raw),
            Some("سلام متن نامعتبر".to_string())
        );
    }

    #[test]
    fn test_malformed_entity_handling() {
        let msg = make_test_message(
            r#"{
                "message_id": 1,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "text": "کوتاه",
                "entities": [
                    {
                        "type": "url",
                        "offset": 9999,
                        "length": 50
                    },
                    {
                        "type": "text_link",
                        "offset": 0,
                        "length": 5
                    }
                ]
            }"#,
        );

        // Does not panic, ignores invalid entities
        let links = extract_message_links(&msg);
        assert!(links.is_empty());
    }

    #[test]
    fn test_extract_message_links_deduplication() {
        let msg = make_test_message(
            r#"{
                "message_id": 1,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "text": "لینک: https://example.com/file.zip تکرار https://example.com/file.zip",
                "entities": [
                    {
                        "type": "url",
                        "offset": 6,
                        "length": 28
                    }
                ]
            }"#,
        );

        let links = extract_message_links(&msg);
        assert_eq!(links, vec!["https://example.com/file.zip"]);
    }

    #[test]
    fn test_extract_plain_urls_with_brackets_and_punctuation() {
        let text = "بررسی کنید: (https://example.com/1.zip), <https://example.com/2.zip>! و \"https://example.com/3.zip\"";
        let urls = extract_plain_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://example.com/1.zip",
                "https://example.com/2.zip",
                "https://example.com/3.zip"
            ]
        );
    }

    #[test]
    fn test_extract_surge_url_ignores_telegram_links() {
        let msg = make_test_message(
            r#"{
                "message_id": 1,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "text": "کانال تلگرام https://t.me/example و دانلود https://example.com/video.mp4",
                "entities": [
                    {
                        "type": "url",
                        "offset": 13,
                        "length": 21
                    },
                    {
                        "type": "url",
                        "offset": 43,
                        "length": 29
                    }
                ]
            }"#,
        );

        // Even though t.me appears first, extract_surge_url chooses the direct link!
        assert_eq!(
            extract_surge_url(&msg),
            Some("https://example.com/video.mp4".to_string())
        );
    }

    #[test]
    fn test_extract_surge_url_detects_social_platforms() {
        let msg = make_test_message(
            r#"{
                "message_id": 1,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "text": "دانلود از اینستاگرام: https://www.instagram.com/reel/C-12345/"
            }"#,
        );

        assert_eq!(
            extract_surge_url(&msg),
            Some("https://www.instagram.com/reel/C-12345/".to_string())
        );
    }
}
