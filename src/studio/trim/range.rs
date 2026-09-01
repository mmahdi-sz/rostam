use regex::Regex;
use std::sync::LazyLock;

pub const DEFAULT_MAX_CUT_RANGES: usize = 10;

static TIMESTAMP_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<start>(?:\d{1,2}:)?\d{1,2}:\d{2})\s*(?:[-–—~]|->|=>|\bto\b|\bتا\b)\s*(?P<end>(?:\d{1,2}:)?\d{1,2}:\d{2})")
        .expect("Valid timestamp range regex")
});

/// Normalizes Persian (`۰-۹`) and Arabic-Indic (`٠-٩`) digits to ASCII (`0-9`).
pub fn normalize_digits(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '۰'..='۹' => (c as u32 - '۰' as u32 + '0' as u32) as u8 as char,
            '٠'..='٩' => (c as u32 - '٠' as u32 + '0' as u32) as u8 as char,
            other => other,
        })
        .collect()
}

/// Converts timestamp string (`HH:MM:SS` or `MM:SS`) to total seconds.
pub fn parse_timestamp(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    match parts.len() {
        2 => {
            let mins: u64 = parts[0].parse().ok()?;
            let secs: u64 = parts[1].parse().ok()?;
            if secs >= 60 {
                return None;
            }
            Some(mins * 60 + secs)
        }
        3 => {
            let hours: u64 = parts[0].parse().ok()?;
            let mins: u64 = parts[1].parse().ok()?;
            let secs: u64 = parts[2].parse().ok()?;
            if mins >= 60 || secs >= 60 {
                return None;
            }
            Some(hours * 3600 + mins * 60 + secs)
        }
        _ => None,
    }
}

/// Formats seconds into `HH:MM:SS` string.
pub fn format_timestamp(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{hours:02}:{mins:02}:{s:02}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutRange {
    pub start_secs: u64,
    pub end_secs: u64,
    pub raw_line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RangeError {
    #[error("No valid cut ranges found in input")]
    NoValidRanges,
    #[error(
        "Line {line_idx}: Invalid format '{text}' (expected MM:SS - MM:SS or HH:MM:SS - HH:MM:SS)"
    )]
    InvalidFormat { line_idx: usize, text: String },
    #[error("Line {line_idx}: Start time ({start}s) is greater than or equal to end time ({end}s)")]
    StartGteEnd {
        line_idx: usize,
        start: u64,
        end: u64,
    },
    #[error("Line {line_idx}: End time ({end}s) exceeds video duration ({duration}s)")]
    EndExceedsDuration {
        line_idx: usize,
        end: u64,
        duration: u64,
    },
    #[error("Too many cut ranges specified (max {max})")]
    ExceedsMaxRanges { max: usize },
}

/// Parses and validates cut ranges extracted from user input (including embedded in long text).
pub fn parse_cut_ranges(
    input: &str,
    duration_secs: u64,
    max_ranges: usize,
) -> Result<Vec<CutRange>, Vec<RangeError>> {
    let normalized = normalize_digits(input);
    let mut ranges = Vec::new();
    let mut errors = Vec::new();

    let matches: Vec<_> = TIMESTAMP_RANGE_RE.captures_iter(&normalized).collect();

    if matches.is_empty() {
        return Err(vec![RangeError::NoValidRanges]);
    }

    if matches.len() > max_ranges {
        return Err(vec![RangeError::ExceedsMaxRanges { max: max_ranges }]);
    }

    for (match_idx, cap) in matches.iter().enumerate() {
        let line_idx = match_idx + 1;
        let start_str = &cap["start"];
        let end_str = &cap["end"];

        let start_opt = parse_timestamp(start_str);
        let end_opt = parse_timestamp(end_str);

        match (start_opt, end_opt) {
            (Some(start), Some(end)) => {
                let end_clamped = end.min(duration_secs);
                if start >= end_clamped {
                    if start >= end {
                        errors.push(RangeError::StartGteEnd {
                            line_idx,
                            start,
                            end,
                        });
                    } else {
                        errors.push(RangeError::EndExceedsDuration {
                            line_idx,
                            end,
                            duration: duration_secs,
                        });
                    }
                } else {
                    ranges.push(CutRange {
                        start_secs: start,
                        end_secs: end_clamped,
                        raw_line: cap[0].to_string(),
                    });
                }
            }
            _ => {
                errors.push(RangeError::InvalidFormat {
                    line_idx,
                    text: cap[0].to_string(),
                });
            }
        }
    }

    if !errors.is_empty() {
        Err(errors)
    } else {
        Ok(ranges)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_normalize_digits() {
        assert_eq!(
            normalize_digits("۰۰:۰۱:۳۰ - ۰۰:۰۵:۴۵"),
            "00:01:30 - 00:05:45"
        );
        assert_eq!(
            normalize_digits("٠٠:٠١:٣٠ - ٠٠:٠٥:٤٥"),
            "00:01:30 - 00:05:45"
        );
        assert_eq!(
            normalize_digits("00:01:00 - 00:02:00"),
            "00:01:00 - 00:02:00"
        );
    }

    #[test]
    fn test_parse_timestamp() {
        assert_eq!(parse_timestamp("01:30"), Some(90));
        assert_eq!(parse_timestamp("01:02:03"), Some(3723));
        assert_eq!(parse_timestamp("00:00:00"), Some(0));
        assert_eq!(parse_timestamp("00:60"), None);
        assert_eq!(parse_timestamp("invalid"), None);
    }

    #[test]
    fn test_parse_cut_ranges_valid() {
        let input = "00:00 - 00:30\n00:01:00-00:02:00\n\n۰۰:۰۲:۳۰ - ۰۰:۰۳:۰۰";
        let duration = 300;
        let res = parse_cut_ranges(input, duration, 10);
        assert!(res.is_ok());
        let ranges = res.unwrap();
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0].start_secs, 0);
        assert_eq!(ranges[0].end_secs, 30);
        assert_eq!(ranges[1].start_secs, 60);
        assert_eq!(ranges[1].end_secs, 120);
        assert_eq!(ranges[2].start_secs, 150);
        assert_eq!(ranges[2].end_secs, 180);
    }

    #[test]
    fn test_parse_cut_ranges_invalid_bounds() {
        let input = "00:02:00 - 00:01:00\n00:06:00 - 00:10:00";
        let duration = 300; // 5 mins
        let res = parse_cut_ranges(input, duration, 10);
        assert!(res.is_err());
        let errors = res.unwrap_err();
        assert_eq!(errors.len(), 2);
        assert!(matches!(errors[0], RangeError::StartGteEnd { .. }));
        assert!(matches!(errors[1], RangeError::EndExceedsDuration { .. }));
    }

    #[test]
    fn test_parse_cut_ranges_autoclamp() {
        let input = "00:01:00 - 00:10:00";
        let duration = 300; // 5 mins
        let res = parse_cut_ranges(input, duration, 10);
        assert!(res.is_ok());
        let ranges = res.unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start_secs, 60);
        assert_eq!(ranges[0].end_secs, 300);
    }

    #[test]
    fn test_parse_cut_ranges_max_cap() {
        let input = "00:01 - 00:02\n00:02 - 00:03\n00:03 - 00:04";
        let res = parse_cut_ranges(input, 300, 2);
        assert!(res.is_err());
        let errors = res.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], RangeError::ExceedsMaxRanges { max: 2 }));
    }

    #[test]
    fn test_parse_cut_ranges_long_text_extraction() {
        let input = "Hello bot! I want to edit this long video for Youtube.\nHere is the description of the video.\nPlease cut the video at the end:\n00:00:00 - 02:00:00\nEnjoy watching!";
        let duration = 7200; // 2 hours
        let res = parse_cut_ranges(input, duration, 10);
        assert!(res.is_ok());
        let ranges = res.unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start_secs, 0);
        assert_eq!(ranges[0].end_secs, 7200);
    }
}
