use std::collections::{HashMap, HashSet};
use std::time::Duration;

use tokio::process::Command;

use super::trace::log_trace;
use super::types::{
    AudioLanguage, FetchError, SubtitleLanguage, VideoCodec, VideoFormatOption, VideoInfo,
};

const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum YtdlpErrorClassification {
    RateLimited,
    BadCookie(String),
    AgeRestricted(String),
    MembersOnly,
    Other(String),
}

pub fn classify_ytdlp_stderr(stderr: &str) -> YtdlpErrorClassification {
    let lower = stderr.to_ascii_lowercase();

    if lower.contains("http error 429") || lower.contains("too many requests") {
        return YtdlpErrorClassification::RateLimited;
    }
    if lower.contains("members-only") || lower.contains("join this channel") {
        return YtdlpErrorClassification::MembersOnly;
    }
    if lower.contains("sign in to confirm your age")
        || lower.contains("inappropriate for some users")
        || lower.contains("age-restricted")
    {
        let msg = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("Sign in to confirm your age")
            .trim();
        return YtdlpErrorClassification::AgeRestricted(msg.to_string());
    }
    if lower.contains("no such table: moz_cookies")
        || lower.contains("database is locked")
        || lower.contains("could not find cookies")
        || lower.contains("could not find firefox cookies")
        || lower.contains("cookies database")
        || lower.contains("unable to open database file")
        || lower.contains("no cookies found")
        || lower.contains("the page needs to be reloaded")
        || lower.contains("confirm you're not a robot")
        || lower.contains("confirm you’re not a robot")
        || lower.contains("sign in to confirm")
        || lower.contains("sign in to confirm you’re not a bot")
        || lower.contains("sign in to confirm you're not a bot")
    {
        let msg = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("bad cookie")
            .trim();
        return YtdlpErrorClassification::BadCookie(msg.to_string());
    }

    let msg = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("unknown error")
        .trim();
    YtdlpErrorClassification::Other(msg.to_string())
}

pub async fn fetch_video_info(
    trace_id: u64,
    url: &str,
    yt_dlp_browser_spec: &str,
) -> Result<VideoInfo, FetchError> {
    log_trace(
        trace_id,
        "fetch_start",
        &format!("url={url} cookie_spec={yt_dlp_browser_spec}"),
    );
    let child = Command::new("yt-dlp")
        .arg("--js-runtimes")
        .arg(format!("deno:{}", crate::config::deno_path()))
        .arg("--cookies-from-browser")
        .arg(yt_dlp_browser_spec)
        .arg("--extractor-args")
        .arg("youtubetab:skip=authcheck")
        .arg("--dump-single-json")
        .arg("--flat-playlist")
        .arg("--no-download")
        .arg("--no-warnings")
        .arg("--ignore-no-formats-error")
        .arg(url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| FetchError::Other(format!("failed to spawn yt-dlp: {e}")))?;

    // On timeout, dropping this future drops the still-owned `Child`, and
    // `kill_on_drop` above ensures that actually kills the process instead of
    // leaking a yt-dlp/deno process stuck resolving a bad or empty URL.
    let output = match tokio::time::timeout(FETCH_TIMEOUT, child.wait_with_output()).await {
        Ok(result) => {
            result.map_err(|e| FetchError::Other(format!("failed to run yt-dlp: {e}")))?
        }
        Err(_) => {
            log_trace(
                trace_id,
                "fetch_timeout",
                &format!(
                    "no response after {}s, killing yt-dlp",
                    FETCH_TIMEOUT.as_secs()
                ),
            );
            return Err(FetchError::Other(format!(
                "yt-dlp timed out after {}s",
                FETCH_TIMEOUT.as_secs()
            )));
        }
    };
    log_trace(
        trace_id,
        "yt_dlp_exit",
        &format!("status={}", output.status),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    match classify_ytdlp_stderr(&stderr) {
        YtdlpErrorClassification::RateLimited => {
            log_trace(
                trace_id,
                "yt_dlp_rate_limited",
                stderr.lines().last().unwrap_or(""),
            );
            return Err(FetchError::RateLimited);
        }
        YtdlpErrorClassification::BadCookie(msg) | YtdlpErrorClassification::AgeRestricted(msg) => {
            log_trace(
                trace_id,
                "yt_dlp_bad_cookie",
                stderr.lines().last().unwrap_or(&msg),
            );
            return Err(FetchError::BadCookie(msg));
        }
        YtdlpErrorClassification::MembersOnly => {
            log_trace(
                trace_id,
                "yt_dlp_members_only",
                stderr.lines().last().unwrap_or(""),
            );
            return Err(FetchError::Other("members-only".to_string()));
        }
        YtdlpErrorClassification::Other(msg) => {
            if !output.status.success() {
                log_trace(
                    trace_id,
                    "yt_dlp_other_error",
                    stderr.lines().last().unwrap_or(&msg),
                );
                return Err(FetchError::Other(format!(
                    "yt-dlp exited with status {}: {}",
                    output.status, msg
                )));
            }
        }
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| FetchError::Other(format!("failed to parse yt-dlp json: {e}")))?;

    // Detect whether playlist or single video.
    let is_playlist = json
        .get("_type")
        .and_then(|v| v.as_str())
        .map(|t| t == "playlist")
        .unwrap_or(false);

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
    let duration = json
        .get("duration")
        .and_then(|v| v.as_f64())
        .map(|d| d as u64);
    let view_count = json.get("view_count").and_then(|v| v.as_u64());
    let like_count = json.get("like_count").and_then(|v| v.as_u64());
    let upload_date = json
        .get("upload_date")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let thumbnail = json
        .get("thumbnail")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            json.get("thumbnails")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.last())
                .and_then(|t| t.get("url"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .map(|url| {
            // Telegram Bot API send_photo by URL returns 400 "wrong type of the web page content" for vi_webp URLs
            url.replace("/vi_webp/", "/vi/").replace(".webp", ".jpg")
        });
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
    let mut video_formats = extract_video_formats(&json);
    if video_formats.is_empty() && !is_playlist && !yt_dlp_browser_spec.is_empty() {
        log_trace(
            trace_id,
            "yt_dlp_zero_formats_with_cookie",
            &format!("cookie_spec={yt_dlp_browser_spec}"),
        );
        return Err(FetchError::BadCookie(format!(
            "0 formats extracted with cookie {}",
            yt_dlp_browser_spec
        )));
    }
    let mut available_heights = available_heights(&video_formats);

    let mut audio_languages = extract_audio_languages(&json);
    let mut subtitle_languages = extract_subtitle_languages(&json);
    let format_count = json
        .get("formats")
        .and_then(|v| v.as_array())
        .map(|v| v.len())
        .unwrap_or(0);
    let requested_format_count = json
        .get("requested_formats")
        .and_then(|v| v.as_array())
        .map(|v| v.len())
        .unwrap_or(0);
    let codec_summary = codec_summary(&video_formats);
    let audio_summary = audio_languages
        .iter()
        .map(|a| format!("{}{}", a.code, if a.is_original { "*" } else { "" }))
        .collect::<Vec<_>>()
        .join(",");
    let sub_summary = subtitle_languages
        .iter()
        .map(|s| format!("{}{}", s.code, if s.is_auto { "(auto)" } else { "" }))
        .collect::<Vec<_>>()
        .join(",");
    let (playlist_item_count, playlist_items) = if is_playlist {
        match fetch_playlist_items(trace_id, url, yt_dlp_browser_spec).await {
            Ok((count, items)) => {
                log_trace(
                    trace_id,
                    "playlist_items_fetched",
                    &format!("count={count}"),
                );
                (Some(count), items)
            }
            Err(e) => {
                log_trace(trace_id, "playlist_items_fetch_failed", &e.to_string());
                (None, Vec::new())
            }
        }
    } else {
        (None, Vec::new())
    };

    // Playlists lack formats in flat-playlist; fetch first video formats as representative.
    if is_playlist {
        if let Some(first) = playlist_items.first() {
            let video_url = format!("https://www.youtube.com/watch?v={}", first.id);
            match Box::pin(fetch_video_info(trace_id, &video_url, yt_dlp_browser_spec)).await {
                Ok(rep) => {
                    log_trace(
                        trace_id,
                        "playlist_representative_formats",
                        &format!("video_id={} heights={:?}", first.id, rep.available_heights),
                    );
                    video_formats = rep.video_formats;
                    available_heights = rep.available_heights;
                    audio_languages = rep.audio_languages;
                    subtitle_languages = rep.subtitle_languages;
                }
                Err(e) => {
                    log_trace(
                        trace_id,
                        "playlist_representative_fetch_failed",
                        &format!("{e:?}"),
                    );
                }
            }
        }
    }

    if !is_playlist && video_formats.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("database is locked")
            || lower.contains("the page needs to be reloaded")
            || lower.contains("confirm you're not a robot")
            || lower.contains("confirm you’re not a robot")
            || lower.contains("sign in to confirm")
            || lower.contains("no video formats found")
        {
            log_trace(
                trace_id,
                "yt_dlp_empty_formats_bad_cookie",
                stderr.lines().last().unwrap_or("no video formats found"),
            );
            return Err(FetchError::BadCookie(
                stderr
                    .lines()
                    .last()
                    .unwrap_or("no video formats found")
                    .to_string(),
            ));
        }
    }

    log_trace(
        trace_id,
        "fetch_parsed",
        &format!(
            "is_playlist={is_playlist} format_count={format_count} requested_format_count={requested_format_count} heights={available_heights:?} codecs={codec_summary} audio_langs={audio_summary} sub_langs={sub_summary}"
        ),
    );

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
        available_heights,
        video_formats,
        audio_languages,
        subtitle_languages,
        is_playlist,
        playlist_item_count,
        playlist_items,
    })
}

async fn fetch_playlist_items(
    trace_id: u64,
    url: &str,
    yt_dlp_browser_spec: &str,
) -> Result<(usize, Vec<super::types::PlaylistItem>), String> {
    let child = Command::new("yt-dlp")
        .arg("--js-runtimes")
        .arg(format!("deno:{}", crate::config::deno_path()))
        .arg("--cookies-from-browser")
        .arg(yt_dlp_browser_spec)
        .arg("--extractor-args")
        .arg("youtubetab:skip=authcheck")
        .arg("--dump-json")
        .arg("--flat-playlist")
        .arg("--no-download")
        .arg("--no-warnings")
        .arg(url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("failed to spawn yt-dlp for playlist: {e}"))?;

    let output = match tokio::time::timeout(FETCH_TIMEOUT, child.wait_with_output()).await {
        Ok(result) => result.map_err(|e| format!("failed to run yt-dlp for playlist: {e}"))?,
        Err(_) => {
            log_trace(trace_id, "playlist_fetch_timeout", "yt-dlp timed out");
            return Err(format!(
                "yt-dlp timed out after {}s",
                FETCH_TIMEOUT.as_secs()
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log_trace(
            trace_id,
            "playlist_yt_dlp_failed",
            stderr.lines().last().unwrap_or(""),
        );
        return Err(format!(
            "yt-dlp failed: {}",
            stderr.lines().last().unwrap_or("")
        ));
    }

    // --dump-json on a playlist prints NDJSON (one JSON object per line).
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut items = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        {
            if let (Some(id), Some(title)) = (
                entry.get("id").and_then(|v| v.as_str()),
                entry.get("title").and_then(|v| v.as_str()),
            ) {
                let duration = entry
                    .get("duration")
                    .and_then(|v| v.as_f64())
                    .map(|d| d as u64);
                items.push(super::types::PlaylistItem {
                    id: id.to_string(),
                    title: title.to_string(),
                    duration,
                });
            }
        }
    }

    let count = items.len();
    log_trace(trace_id, "playlist_items_parsed", &format!("count={count}"));
    Ok((count, items))
}

fn extract_audio_languages(json: &serde_json::Value) -> Vec<AudioLanguage> {
    let mut by_lang: HashMap<String, bool> = HashMap::new();
    let mut collect = |format: &serde_json::Value| {
        let Some(lang) = format.get("language").and_then(|v| v.as_str()) else {
            return;
        };
        if lang.is_empty() {
            return;
        }
        let note = format
            .get("format_note")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let lang_pref = format
            .get("language_preference")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        let is_original = note.contains("original") || lang_pref >= 10;
        let entry = by_lang.entry(lang.to_string()).or_insert(false);
        if is_original {
            *entry = true;
        }
    };
    if let Some(formats) = json.get("formats").and_then(|v| v.as_array()) {
        for format in formats {
            collect(format);
        }
    }
    if let Some(formats) = json.get("requested_formats").and_then(|v| v.as_array()) {
        for format in formats {
            collect(format);
        }
    }
    let mut out: Vec<AudioLanguage> = by_lang
        .into_iter()
        .map(|(code, is_original)| AudioLanguage { code, is_original })
        .collect();
    out.sort_by(|a, b| {
        b.is_original
            .cmp(&a.is_original)
            .then_with(|| a.code.cmp(&b.code))
    });
    out
}

fn extract_subtitle_languages(json: &serde_json::Value) -> Vec<SubtitleLanguage> {
    let mut subs: HashMap<String, bool> = HashMap::new();
    if let Some(obj) = json.get("subtitles").and_then(|v| v.as_object()) {
        for key in obj.keys() {
            if key.is_empty() {
                continue;
            }
            subs.insert(key.clone(), false);
        }
    }
    if let Some(obj) = json.get("automatic_captions").and_then(|v| v.as_object()) {
        for key in obj.keys() {
            if key.is_empty() {
                continue;
            }
            subs.entry(key.clone()).or_insert(true);
        }
    }
    let mut out: Vec<SubtitleLanguage> = subs
        .into_iter()
        .map(|(code, is_auto)| SubtitleLanguage { code, is_auto })
        .collect();
    out.sort_by(|a, b| a.is_auto.cmp(&b.is_auto).then_with(|| a.code.cmp(&b.code)));
    out
}

fn extract_video_formats(json: &serde_json::Value) -> Vec<VideoFormatOption> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    if let Some(formats) = json.get("formats").and_then(|v| v.as_array()) {
        for format in formats {
            collect_video_format(format, &mut seen, &mut out);
        }
    }
    if let Some(requested_formats) = json.get("requested_formats").and_then(|v| v.as_array()) {
        for format in requested_formats {
            collect_video_format(format, &mut seen, &mut out);
        }
    }
    collect_video_format(json, &mut seen, &mut out);
    out.sort_by(|left, right| right.height.cmp(&left.height));
    out
}

fn collect_video_format(
    format: &serde_json::Value,
    seen: &mut HashSet<(u32, VideoCodec)>,
    out: &mut Vec<VideoFormatOption>,
) {
    let Some(height) = format.get("height").and_then(|v| v.as_u64()) else {
        return;
    };
    let Some(vcodec) = format.get("vcodec").and_then(|v| v.as_str()) else {
        return;
    };
    let Some(codec) = parse_video_codec(vcodec) else {
        return;
    };
    if height > u32::MAX as u64 {
        return;
    }
    // Non-16:9 videos (e.g. 2:1 cinematic) report off-tier pixel heights like
    // 960 instead of 1080, so exact-height matching in quality_options() would
    // drop every format and hide all download buttons. Snap to the nearest
    // standard tier; the real format_id (below) is what actually downloads.
    let height = normalize_height(height as u32);
    if !seen.insert((height, codec)) {
        return;
    }
    let format_id = format
        .get("format_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let bitrate = format
        .get("tbr")
        .or_else(|| format.get("vbr"))
        .and_then(|v| v.as_f64());
    out.push(VideoFormatOption {
        height,
        codec,
        format_id,
        bitrate,
    });
}

/// Snap a raw pixel height to the nearest standard quality tier, so off-tier
/// resolutions (from non-16:9 aspect ratios) still map to a labelled button.
fn normalize_height(h: u32) -> u32 {
    const STD: [u32; 9] = [144, 240, 360, 480, 720, 1080, 1440, 2160, 4320];
    STD.into_iter().min_by_key(|&s| s.abs_diff(h)).unwrap()
}

fn parse_video_codec(vcodec: &str) -> Option<VideoCodec> {
    let vcodec = vcodec.to_ascii_lowercase();
    if vcodec.starts_with("avc1") {
        Some(VideoCodec::H264)
    } else if vcodec.starts_with("hvc1") || vcodec.starts_with("dvh1") {
        Some(VideoCodec::H265)
    } else if vcodec == "vp9" || vcodec.starts_with("vp09") {
        Some(VideoCodec::Vp9)
    } else if vcodec.starts_with("av01") {
        Some(VideoCodec::Av1)
    } else {
        None
    }
}

fn available_heights(video_formats: &[VideoFormatOption]) -> Vec<u32> {
    let mut heights: Vec<u32> = video_formats.iter().map(|format| format.height).collect();
    heights.sort_unstable();
    heights.dedup();
    heights
}

fn codec_summary(video_formats: &[VideoFormatOption]) -> String {
    video_formats
        .iter()
        .map(|format| {
            format!(
                "{}:{}:{}",
                format.height,
                format.codec.key(),
                format.format_id
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_age_restriction() {
        let stderr = "ERROR: [youtube] sgHbCZyBrgU: Sign in to confirm your age. This video may be inappropriate for some users.";
        assert!(matches!(
            classify_ytdlp_stderr(stderr),
            YtdlpErrorClassification::AgeRestricted(_)
        ));
    }

    #[test]
    fn test_classify_429() {
        let stderr = "HTTP Error 429: Too Many Requests";
        assert_eq!(
            classify_ytdlp_stderr(stderr),
            YtdlpErrorClassification::RateLimited
        );
    }

    #[test]
    fn test_classify_bad_cookie() {
        let stderr = "ERROR: [youtube] The page needs to be reloaded.";
        assert!(matches!(
            classify_ytdlp_stderr(stderr),
            YtdlpErrorClassification::BadCookie(_)
        ));
    }
}

