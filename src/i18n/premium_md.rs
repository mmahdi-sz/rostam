use super::emoji_map::EMOJI_MAP;
use super::lookup::t;

/// Wrap known emojis in `<tg-emoji emoji-id="...">EMOJI</tg-emoji>` tags so a
/// message sent with `ParseMode::Html` renders them as premium custom emoji.
///
/// 🔥 is randomized across `emoji.panel.icons.fire1..fire4` (whichever are set),
/// so repeated fires in the same text use different premium variants. Content
/// inside HTML tags (`<...>`) is left untouched so existing markup is preserved.
pub fn apply_premium_to_html(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 128);
    let mut rest = text;
    let mut in_tag = false;
    'outer: while !rest.is_empty() {
        let Some(c) = rest.chars().next() else {
            break;
        };

        if in_tag {
            result.push(c);
            if c == '>' {
                in_tag = false;
            }
            rest = &rest[c.len_utf8()..];
            continue;
        }
        if c == '<' {
            in_tag = true;
            result.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        }

        // 🔥 — random premium variant.
        if rest.starts_with('🔥') {
            if let Some(id) = random_fire_id() {
                result.push_str(&format!("<tg-emoji emoji-id=\"{id}\">🔥</tg-emoji>"));
                rest = &rest['🔥'.len_utf8()..];
                continue;
            }
        }

        for (emoji_str, icon_key) in EMOJI_MAP {
            if rest.starts_with(emoji_str) {
                let icon_id = t(&format!("emoji.panel.icons.{icon_key}"));
                if !icon_id.is_empty() && !icon_id.starts_with('!') {
                    result.push_str(&format!(
                        "<tg-emoji emoji-id=\"{icon_id}\">{emoji_str}</tg-emoji>"
                    ));
                    rest = &rest[emoji_str.len()..];
                    continue 'outer;
                }
            }
        }

        result.push(c);
        rest = &rest[c.len_utf8()..];
    }
    result
}

/// Pick a random non-empty `emoji.panel.icons.fire{1..4}` id, or None if none set.
fn random_fire_id() -> Option<String> {
    use rand::Rng;
    let ids: Vec<String> = (1..=4)
        .map(|i| t(&format!("emoji.panel.icons.fire{i}")))
        .filter(|s| !s.is_empty() && !s.starts_with('!'))
        .collect();
    if ids.is_empty() {
        return None;
    }
    let idx = rand::thread_rng().gen_range(0..ids.len());
    Some(ids[idx].clone())
}

/// Wrap known emojis in MarkdownV2 custom-emoji syntax
/// (`![😀](tg://emoji?id=…)`).
///
/// Text that is *already* a custom-emoji span is copied verbatim: several i18n
/// strings (e.g. `fc.welcome`) ship the span pre-written, and re-wrapping the
/// emoji inside it produced `![![📂](tg://emoji?id=A)](tg://emoji?id=B)`, which
/// Telegram rejects with `Bad Request: ENTITY_TEXT_INVALID` — the whole file
/// compression menu failed to render.
pub fn apply_premium_to_md(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 64);
    let mut rest = text;
    'outer: while !rest.is_empty() {
        // Pass an existing `![…](tg://emoji?id=…)` span through untouched.
        if let Some(span) = existing_emoji_span(rest) {
            result.push_str(&rest[..span]);
            rest = &rest[span..];
            continue;
        }

        for (emoji_str, icon_key) in EMOJI_MAP {
            if rest.starts_with(emoji_str) {
                let icon_id = t(&format!("emoji.panel.icons.{icon_key}"));
                if !icon_id.is_empty() && !icon_id.starts_with('!') {
                    result.push_str(&format!("![{emoji_str}](tg://emoji?id={icon_id})"));
                    rest = &rest[emoji_str.len()..];
                    continue 'outer;
                }
            }
        }
        let Some(c) = rest.chars().next() else {
            break;
        };
        result.push(c);
        rest = &rest[c.len_utf8()..];
    }
    result
}

/// If `s` starts with a custom-emoji span, return its byte length.
fn existing_emoji_span(s: &str) -> Option<usize> {
    let after_bracket = s.strip_prefix("![")?;
    let close = after_bracket.find("](tg://emoji?id=")?;
    let tail = &after_bracket[close + "](tg://emoji?id=".len()..];
    let end = tail.find(')')?;
    Some(2 + close + "](tg://emoji?id=".len() + end + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_written_span_is_not_wrapped_again() {
        let src = "• ![📂](tg://emoji?id=5341492148468465410) *ZIP*";
        assert_eq!(apply_premium_to_md(src), src);
    }

    #[test]
    fn escaped_bang_is_not_emitted() {
        // `\![…]` renders as literal text, not a custom emoji entity.
        assert!(!apply_premium_to_md("📁 test").contains("\\!["));
    }
}
