use tokio::process::Command;

use crate::i18n::t;

#[derive(Debug)]
pub struct VideoInfo {
    pub title: String,
    pub channel: String,
    pub duration: Option<u64>,
    pub view_count: Option<u64>,
    pub like_count: Option<u64>,
    pub upload_date: Option<String>,
    pub thumbnail: Option<String>,
    pub webpage_url: String,
    pub description: Option<String>,
}

#[derive(Debug)]
pub enum FetchError {
    RateLimited,
    BadCookie(String),
    Other(String),
}

pub fn extract_youtube_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();

    for token in text.split(|c: char| c.is_whitespace()) {
        let token = token.trim_matches(|c: char| {
            matches!(c, '<' | '>' | '"' | '\'' | ',' | ';' | '!' | '?' | ')' | '(')
        });

        if token.is_empty() {
            continue;
        }

        let lower = token.to_ascii_lowercase();
        let normalized = if lower.starts_with("http://") || lower.starts_with("https://") {
            token.to_string()
        } else if lower.starts_with("www.youtube.com")
            || lower.starts_with("youtube.com")
            || lower.starts_with("m.youtube.com")
            || lower.starts_with("youtu.be")
        {
            format!("https://{}", token)
        } else {
            continue;
        };

        let host_part = normalized
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("")
            .to_ascii_lowercase();

        let is_yt = host_part == "youtu.be"
            || host_part == "youtube.com"
            || host_part == "www.youtube.com"
            || host_part == "m.youtube.com"
            || host_part == "music.youtube.com";

        if is_yt && !urls.contains(&normalized) {
            urls.push(normalized);
        }
    }

    urls
}

pub async fn fetch_video_info(
    url: &str,
    yt_dlp_browser_spec: &str,
) -> Result<VideoInfo, FetchError> {
    let output = Command::new("yt-dlp")
        .arg("--cookies-from-browser")
        .arg(yt_dlp_browser_spec)
        .arg("--dump-single-json")
        .arg("--no-download")
        .arg("--no-warnings")
        .arg("--no-playlist")
        .arg("--ignore-no-formats-error")
        .arg(url)
        .output()
        .await
        .map_err(|e| FetchError::Other(format!("failed to spawn yt-dlp: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("http error 429") || lower.contains("too many requests") {
            return Err(FetchError::RateLimited);
        }
        if lower.contains("no such table: moz_cookies")
            || lower.contains("database is locked")
            || lower.contains("could not find cookies")
            || lower.contains("unable to open database file")
            || lower.contains("no cookies found")
        {
            return Err(FetchError::BadCookie(
                stderr.lines().last().unwrap_or("").to_string(),
            ));
        }
        return Err(FetchError::Other(format!(
            "yt-dlp exited with status {}: {}",
            output.status,
            stderr.lines().last().unwrap_or("").to_string()
        )));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| FetchError::Other(format!("failed to parse yt-dlp json: {e}")))?;

    let title = json
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let channel = json
        .get("channel")
        .or_else(|| json.get("uploader"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let duration = json.get("duration").and_then(|v| v.as_f64()).map(|d| d as u64);
    let view_count = json.get("view_count").and_then(|v| v.as_u64());
    let like_count = json.get("like_count").and_then(|v| v.as_u64());
    let upload_date = json
        .get("upload_date")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let thumbnail = json
        .get("thumbnail")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let webpage_url = json
        .get("webpage_url")
        .and_then(|v| v.as_str())
        .unwrap_or(url)
        .to_string();

    let description = json
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty());

    Ok(VideoInfo {
        title,
        channel,
        duration,
        view_count,
        like_count,
        upload_date,
        thumbnail,
        webpage_url,
        description,
    })
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
        if i > 0 && (len - i) % 3 == 0 {
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

fn gregorian_to_jalali(gy: i32, gm: i32, gd: i32) -> (i32, i32, i32) {
    let g_d_m = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let gy2 = if gm > 2 { gy + 1 } else { gy };
    let mut days = 355_666 + (365 * gy) + ((gy2 + 3) / 4) - ((gy2 + 99) / 100)
        + ((gy2 + 399) / 400)
        + gd
        + g_d_m[(gm - 1) as usize];
    let mut jy = -1595 + 33 * (days / 12053);
    days %= 12053;
    jy += 4 * (days / 1461);
    days %= 1461;
    if days > 365 {
        jy += (days - 1) / 365;
        days = (days - 1) % 365;
    }
    let (jm, jd) = if days < 186 {
        (1 + days / 31, 1 + days % 31)
    } else {
        (7 + (days - 186) / 30, 1 + (days - 186) % 30)
    };
    (jy, jm, jd)
}

pub fn build_caption(info: &VideoInfo) -> String {
    let missing = escape_markdown_v2(&t("youtube.caption.missing"));
    let title = escape_markdown_v2(&info.title);
    let channel = escape_markdown_v2(&info.channel);
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
