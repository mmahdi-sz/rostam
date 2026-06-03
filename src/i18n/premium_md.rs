use super::lookup::t;
use super::emoji_map::EMOJI_MAP;

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
