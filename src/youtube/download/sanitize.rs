pub fn quality_label_for(height: u32) -> String {
    let key = format!("youtube.quality.buttons.{height}");
    let label = crate::i18n::t(&key);
    if label.starts_with('!') {
        format!("{height}p")
    } else {
        label
    }
}

/// Create a clean, safe OS filename for downloaded video files.
/// Ensures technical info (e.g. [1080p AV1 1498kbps]) is strictly preserved,
/// while the title is sanitized and truncated if the total byte length exceeds 245 bytes.
pub fn sanitize_video_filename(
    title: &str,
    quality_label: &str,
    codec_name: &str,
    bitrate_str: &str,
    ext: &str,
) -> String {
    let tech_info = if bitrate_str != "?" && !bitrate_str.is_empty() {
        format!("[{quality_label} {codec_name} {bitrate_str}kbps]")
    } else {
        format!("[{quality_label} {codec_name}]")
    };
    let ext_with_dot = format!(".{ext}");
    let suffix = format!(" {tech_info}{ext_with_dot}");

    const MAX_TOTAL_BYTES: usize = 245;
    let suffix_bytes = suffix.len();
    let max_title_bytes = if MAX_TOTAL_BYTES > suffix_bytes {
        MAX_TOTAL_BYTES - suffix_bytes
    } else {
        50
    };

    let mut clean_title = String::with_capacity(title.len());
    for c in title.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => {
                clean_title.push(' ');
            }
            _ => clean_title.push(c),
        }
    }

    let mut normalized_title = clean_title.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized_title.is_empty() {
        normalized_title = "video".to_string();
    }

    if normalized_title.len() > max_title_bytes {
        let mut byte_count = 0;
        let mut cutoff = normalized_title.len();

        for (idx, ch) in normalized_title.char_indices() {
            let ch_len = ch.len_utf8();
            if byte_count + ch_len > max_title_bytes {
                cutoff = idx;
                break;
            }
            byte_count += ch_len;
        }

        normalized_title.truncate(cutoff);
        normalized_title = normalized_title.trim_end().to_string();
    }

    format!("{normalized_title}{suffix}")
}

/// Create a clean, safe OS filename for downloaded audio files.
pub fn sanitize_audio_filename(title: &str, quality_label: &str, ext: &str) -> String {
    let tech_info = format!("[{quality_label}]");
    let ext_with_dot = format!(".{ext}");
    let suffix = format!(" {tech_info}{ext_with_dot}");

    const MAX_TOTAL_BYTES: usize = 245;
    let suffix_bytes = suffix.len();
    let max_title_bytes = if MAX_TOTAL_BYTES > suffix_bytes {
        MAX_TOTAL_BYTES - suffix_bytes
    } else {
        50
    };

    let mut clean_title = String::with_capacity(title.len());
    for c in title.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => {
                clean_title.push(' ');
            }
            _ => clean_title.push(c),
        }
    }

    let mut normalized_title = clean_title.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized_title.is_empty() {
        normalized_title = "audio".to_string();
    }

    if normalized_title.len() > max_title_bytes {
        let mut byte_count = 0;
        let mut cutoff = normalized_title.len();

        for (idx, ch) in normalized_title.char_indices() {
            let ch_len = ch.len_utf8();
            if byte_count + ch_len > max_title_bytes {
                cutoff = idx;
                break;
            }
            byte_count += ch_len;
        }

        normalized_title.truncate(cutoff);
        normalized_title = normalized_title.trim_end().to_string();
    }

    format!("{normalized_title}{suffix}")
}
