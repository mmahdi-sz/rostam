use super::lookup::t;
use super::emoji_map::EMOJI_MAP;

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
        let c = rest.chars().next().unwrap();

        if in_tag {
            result.push(c);
            if c == '>' { in_tag = false; }
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
                if !icon_id.is_empty() {
                    result.push_str(&format!("<tg-emoji emoji-id=\"{icon_id}\">{emoji_str}</tg-emoji>"));
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
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return None;
    }
    let idx = rand::thread_rng().gen_range(0..ids.len());
    Some(ids[idx].clone())
}

pub fn apply_premium_to_md(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 64);
    let mut rest = text;
    'outer: while !rest.is_empty() {
        for (emoji_str, icon_key) in EMOJI_MAP {
            if rest.starts_with(emoji_str) {
                let icon_id = t(&format!("emoji.panel.icons.{icon_key}"));
                if !icon_id.is_empty() {
                    result.push_str(&format!("![{}](tg://emoji?id={})", emoji_str, icon_id));
                    rest = &rest[emoji_str.len()..];
                    continue 'outer;
                }
            }
        }
        let c = rest.chars().next().unwrap();
        result.push(c);
        rest = &rest[c.len_utf8()..];
    }
    result
}
