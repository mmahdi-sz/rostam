use std::collections::{HashMap, HashSet};

use tokio::process::Command;

use super::trace::log_trace;
use super::types::{
    AudioLanguage, FetchError, SubtitleLanguage, VideoCodec, VideoFormatOption, VideoInfo,
};

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
    let output = Command::new("yt-dlp")
        .arg("--js-runtimes")
        .arg("deno:/root/.deno/bin/deno")
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
    log_trace(
        trace_id,
        "yt_dlp_exit",
        &format!("status={}", output.status),
    );

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("http error 429") || lower.contains("too many requests") {
            log_trace(
                trace_id,
                "yt_dlp_rate_limited",
                stderr.lines().last().unwrap_or(""),
            );
            return Err(FetchError::RateLimited);
        }
        if lower.contains("no such table: moz_cookies")
            || lower.contains("database is locked")
            || lower.contains("could not find cookies")
            || lower.contains("unable to open database file")
            || lower.contains("no cookies found")
        {
            log_trace(
                trace_id,
                "yt_dlp_bad_cookie",
                stderr.lines().last().unwrap_or(""),
            );
            return Err(FetchError::BadCookie(
                stderr.lines().last().unwrap_or("").to_string(),
            ));
        }
        log_trace(
            trace_id,
            "yt_dlp_other_error",
            stderr.lines().last().unwrap_or(""),
        );
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
    let video_formats = extract_video_formats(&json);
    let available_heights = available_heights(&video_formats);
    let audio_languages = extract_audio_languages(&json);
    let subtitle_languages = extract_subtitle_languages(&json);
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
    log_trace(
        trace_id,
        "fetch_parsed",
        &format!(
            "format_count={format_count} requested_format_count={requested_format_count} heights={available_heights:?} codecs={codec_summary} audio_langs={audio_summary} sub_langs={sub_summary}"
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
    })
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
    out.sort_by(|a, b| {
        a.is_auto
            .cmp(&b.is_auto)
            .then_with(|| a.code.cmp(&b.code))
    });
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
    let height = height as u32;
    if !seen.insert((height, codec)) {
        return;
    }
    let format_id = format
        .get("format_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    out.push(VideoFormatOption {
        height,
        codec,
        format_id,
    });
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
