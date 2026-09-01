use crate::i18n::{apply_premium_to_md, t};

use super::jalali::gregorian_to_jalali;
use super::types::VideoInfo;

pub fn escape_markdown_v2(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '_' | '*'
                | '['
                | ']'
                | '('
                | ')'
                | '~'
                | '`'
                | '>'
                | '#'
                | '+'
                | '-'
                | '—'
                | '–'
                | '='
                | '|'
                | '{'
                | '}'
                | '.'
                | '!'
                | '\\'
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
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

pub fn format_upload_date(yyyymmdd: &str) -> String {
    if yyyymmdd.len() != 8 {
        return yyyymmdd.to_string();
    }
    let gy: i32 = match yyyymmdd[0..4].parse() {
        Ok(v) => v,
        Err(_) => return yyyymmdd.to_string(),
    };
    let gm: i32 = match yyyymmdd[4..6].parse() {
        Ok(v) => v,
        Err(_) => return yyyymmdd.to_string(),
    };
    let gd: i32 = match yyyymmdd[6..8].parse() {
        Ok(v) => v,
        Err(_) => return yyyymmdd.to_string(),
    };
    let (jy, jm, jd) = gregorian_to_jalali(gy, gm, gd);
    format!("{jy:04}/{jm:02}/{jd:02}")
}

pub fn build_caption(info: &VideoInfo) -> String {
    let missing = escape_markdown_v2(&t("youtube.caption.missing"));
    let title = escape_markdown_v2(&info.title);
    let channel = escape_markdown_v2(&info.channel);
    let url = info.webpage_url.replace(')', "%29").replace('\\', "");
    let link_text = escape_markdown_v2(&t("youtube.caption.link_text"));
    let channel_label = escape_markdown_v2(&t("youtube.caption.channel_label"));

    if info.is_playlist {
        let count = info
            .playlist_item_count
            .unwrap_or(info.playlist_items.len());
        let count_str = escape_markdown_v2(&format_count(count as u64));
        let count_label = escape_markdown_v2(&t("youtube.caption.video_count_label"));
        let views = info
            .view_count
            .map(format_count)
            .map(|s| escape_markdown_v2(&s));

        let views_line = if let Some(v) = views {
            let views_label = escape_markdown_v2(&t("youtube.caption.views_label"));
            format!("\n👁 *{views_label}* {v}")
        } else {
            String::new()
        };

        let raw = format!(
            "🗂 *{title}*\n\n\
             👤 *{channel_label}* {channel}\n\
             🔢 *{count_label}* {count_str}{views_line}\n\n\
             🔗 [{link_text}]({url})"
        );
        return apply_premium_to_md(&raw);
    }

    let duration = info
        .duration
        .map(format_duration)
        .map(|s| escape_markdown_v2(&s))
        .unwrap_or_else(|| missing.clone());
    let views = info
        .view_count
        .map(format_count)
        .map(|s| escape_markdown_v2(&s))
        .unwrap_or_else(|| missing.clone());
    let likes = info
        .like_count
        .map(format_count)
        .map(|s| escape_markdown_v2(&s))
        .unwrap_or_else(|| missing.clone());
    let date = info
        .upload_date
        .as_deref()
        .map(format_upload_date)
        .map(|s| escape_markdown_v2(&s))
        .unwrap_or_else(|| missing.clone());

    let duration_label = escape_markdown_v2(&t("youtube.caption.duration_label"));
    let views_label = escape_markdown_v2(&t("youtube.caption.views_label"));
    let likes_label = escape_markdown_v2(&t("youtube.caption.likes_label"));
    let date_label = escape_markdown_v2(&t("youtube.caption.date_label"));

    let raw = format!(
        "🎬 *{title}*\n\n\
         👤 *{channel_label}* {channel}\n\
         ⏱ *{duration_label}* {duration}\n\
         👁 *{views_label}* {views}\n\
         👍 *{likes_label}* {likes}\n\
         📅 *{date_label}* {date}\n\n\
         🔗 [{link_text}]({url})"
    );
    apply_premium_to_md(&raw)
}

pub fn build_description_blockquotes(description: &str) -> Vec<String> {
    const MAX_RAW_PER_CHUNK: usize = 1800; // post-escape can be 2×; keep total <4096
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for line in description.lines() {
        let candidate_len = current.len() + line.len() + 1;
        if !current.is_empty() && candidate_len > MAX_RAW_PER_CHUNK {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
        .into_iter()
        .map(|chunk| {
            let mut out = String::new();
            for (i, line) in chunk.lines().enumerate() {
                let escaped = escape_markdown_v2(line);
                if i == 0 {
                    out.push_str("**>");
                } else {
                    out.push('\n');
                    out.push('>');
                }
                out.push_str(&escaped);
            }
            out.push_str("||");
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(45), "0:45");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(125), "2:05");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(3661), "1:01:01");
    }

    #[test]
    fn test_format_count() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234567), "1,234,567");
    }

    #[test]
    fn test_escape_markdown_v2() {
        assert_eq!(escape_markdown_v2("hello_world"), "hello\\_world");
        assert_eq!(escape_markdown_v2("test. [link]"), "test\\. \\[link\\]");
    }

    #[test]
    fn test_build_description_blockquotes() {
        let desc = "Line 1\nLine 2";
        let chunks = build_description_blockquotes(desc);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("Line 1"));
    }

    #[test]
    fn test_build_caption_playlist() {
        let info = VideoInfo {
            title: "English Playlist".to_string(),
            channel: "Teacher John".to_string(),
            duration: None,
            view_count: Some(15000),
            like_count: None,
            upload_date: None,
            thumbnail: None,
            webpage_url: "https://www.youtube.com/playlist?list=PL123".to_string(),
            description: None,
            available_heights: vec![],
            video_formats: vec![],
            audio_languages: vec![],
            subtitle_languages: vec![],
            is_playlist: true,
            playlist_item_count: Some(25),
            playlist_items: vec![],
        };
        let caption = build_caption(&info);
        assert!(caption.contains("🗂"));
        assert!(caption.contains("Teacher John"));
        assert!(caption.contains("25"));
        assert!(caption.contains("15,000"));
    }
}
