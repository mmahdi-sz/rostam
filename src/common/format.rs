//! Shared formatting utilities for bytes, durations, and timestamps.

/// Formats a byte size into human-readable representation (e.g. 1.25 MB, 500 KB).
pub fn fmt_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * KB;
    const GB: f64 = 1024.0 * MB;

    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Formats seconds into mm:ss or hh:mm:ss clock notation.
pub fn format_clock(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Formats seconds into human-readable string (e.g. "1h 23m" or "45s").
pub fn fmt_secs(secs: i64) -> String {
    if secs < 0 {
        return "0s".to_string();
    }
    let s = secs as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {sec}s")
    } else {
        format!("{sec}s")
    }
}

/// Create a clean, safe OS filename for a music track (e.g. "Artist - Title.mp3").
/// Replaces illegal filesystem characters (/ \ : * ? " < > | \0) with spaces,
/// collapses whitespace, and bounds UTF-8 byte length to prevent filesystem errors.
pub fn safe_music_filename(artist: &str, title: &str) -> String {
    let raw = if artist.trim().is_empty() {
        title.trim().to_string()
    } else if title.trim().is_empty() {
        artist.trim().to_string()
    } else {
        format!("{} - {}", artist.trim(), title.trim())
    };

    let mut clean = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => clean.push(' '),
            _ => clean.push(c),
        }
    }
    let normalized = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let base = if normalized.is_empty() {
        "track".to_string()
    } else {
        let mut byte_count = 0;
        let mut cutoff = normalized.len();
        for (idx, ch) in normalized.char_indices() {
            let ch_len = ch.len_utf8();
            if byte_count + ch_len > 180 {
                cutoff = idx;
                break;
            }
            byte_count += ch_len;
        }
        normalized[..cutoff].trim_end().to_string()
    };
    format!("{base}.mp3")
}

/// Create a clean, safe OS base filename for an archive (e.g. "Album Name").
pub fn safe_archive_stem(title: &str) -> String {
    let mut clean = String::with_capacity(title.len());
    for c in title.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => clean.push(' '),
            _ => clean.push(c),
        }
    }
    let normalized = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        "archive".to_string()
    } else {
        let mut byte_count = 0;
        let mut cutoff = normalized.len();
        for (idx, ch) in normalized.char_indices() {
            let ch_len = ch.len_utf8();
            if byte_count + ch_len > 100 {
                cutoff = idx;
                break;
            }
            byte_count += ch_len;
        }
        normalized[..cutoff].trim_end().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmt_bytes() {
        assert_eq!(fmt_bytes(500), "500 B");
        assert_eq!(fmt_bytes(1024), "1.0 KB");
        assert_eq!(fmt_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(fmt_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_format_clock() {
        assert_eq!(format_clock(45), "00:45");
        assert_eq!(format_clock(65), "01:05");
        assert_eq!(format_clock(3665), "01:01:05");
    }

    #[test]
    fn test_safe_music_filename() {
        assert_eq!(
            safe_music_filename("Shadmehr Aghili", "Taghdir"),
            "Shadmehr Aghili - Taghdir.mp3"
        );
        assert_eq!(
            safe_music_filename("", "Bohemian Rhapsody"),
            "Bohemian Rhapsody.mp3"
        );
        assert_eq!(
            safe_music_filename("AC/DC", "Back in Black: Special*Edition?"),
            "AC DC - Back in Black Special Edition.mp3"
        );
        assert_eq!(safe_music_filename("", ""), "track.mp3");
    }

    #[test]
    fn test_safe_archive_stem() {
        assert_eq!(
            safe_archive_stem("The Dark Side of the Moon: Remastered"),
            "The Dark Side of the Moon Remastered"
        );
        assert_eq!(safe_archive_stem(""), "archive");
    }
}
