use crate::i18n::t;

use super::jalali::gregorian_to_jalali;
use super::types::VideoInfo;

pub fn escape_markdown_v2(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#'
                | '+' | '-' | '=' | '|' | '{' | '}' | '.' | '!' | '\\'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

pub fn format_duration(seconds: u64) -> String {
    let h = seconds / 3600;
    let m = (seconds % 3600) / 60;
    let s = seconds % 60;
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 { out.push(','); }
        out.push(*b as char);
    }
    out
}

pub fn format_upload_date(yyyymmdd: &str) -> String {
    if yyyymmdd.len() != 8 { return yyyymmdd.to_string(); }
    let gy: i32 = match yyyymmdd[0..4].parse() { Ok(v) => v, Err(_) => return yyyymmdd.to_string() };
    let gm: i32 = match yyyymmdd[4..6].parse() { Ok(v) => v, Err(_) => return yyyymmdd.to_string() };
    let gd: i32 = match yyyymmdd[6..8].parse() { Ok(v) => v, Err(_) => return yyyymmdd.to_string() };
    let (jy, jm, jd) = gregorian_to_jalali(gy, gm, gd);
    format!("{jy:04}/{jm:02}/{jd:02}")
}

pub fn build_caption(info: &VideoInfo) -> String {
    let missing = escape_markdown_v2(&t("youtube.caption.missing"));
    let title = escape_markdown_v2(&info.title);
    let channel = escape_markdown_v2(&info.channel);
    let duration = info.duration.map(format_duration).map(|s| escape_markdown_v2(&s)).unwrap_or_else(|| missing.clone());
    let views = info.view_count.map(format_count).map(|s| escape_markdown_v2(&s)).unwrap_or_else(|| missing.clone());
    let likes = info.like_count.map(format_count).map(|s| escape_markdown_v2(&s)).unwrap_or_else(|| missing.clone());
    let date = info.upload_date.as_deref().map(format_upload_date).map(|s| escape_markdown_v2(&s)).unwrap_or_else(|| missing.clone());
    let url = info.webpage_url.replace(')', "%29").replace('\\', "");

    let channel_label = escape_markdown_v2(&t("youtube.caption.channel_label"));
    let duration_label = escape_markdown_v2(&t("youtube.caption.duration_label"));
    let views_label = escape_markdown_v2(&t("youtube.caption.views_label"));
    let likes_label = escape_markdown_v2(&t("youtube.caption.likes_label"));
    let date_label = escape_markdown_v2(&t("youtube.caption.date_label"));
    let link_text = escape_markdown_v2(&t("youtube.caption.link_text"));

    format!(
        "🎬 *{title}*\n\n\
         👤 *{channel_label}* {channel}\n\
         ⏱ *{duration_label}* {duration}\n\
         👁 *{views_label}* {views}\n\
         👍 *{likes_label}* {likes}\n\
         📅 *{date_label}* {date}\n\n\
         🔗 [{link_text}]({url})"
    )
}

pub fn build_description_blockquotes(description: &str) -> Vec<String> {
    const MAX_RAW_PER_CHUNK: usize = 3800;
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for line in description.lines() {
        let candidate_len = current.len() + line.len() + 1;
        if !current.is_empty() && candidate_len > MAX_RAW_PER_CHUNK {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() { current.push('\n'); }
        current.push_str(line);
    }
    if !current.is_empty() { chunks.push(current); }

    chunks.into_iter().map(|chunk| {
        let mut out = String::new();
        for (i, line) in chunk.lines().enumerate() {
            let escaped = escape_markdown_v2(line);
            if i == 0 { out.push_str("**>"); } else { out.push('\n'); out.push('>'); }
            out.push_str(&escaped);
        }
        out.push_str("||");
        out
    }).collect()
}
