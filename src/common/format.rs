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
}
