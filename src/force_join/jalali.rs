use chrono::{Datelike, Timelike};
use chrono_tz::Asia::Tehran;

use crate::youtube::jalali::gregorian_to_jalali;

pub(crate) fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Persian/Arabic digits → English, for parsing admin numeric input.
pub(crate) fn to_en_digits(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '۰'..='۹' => ((c as u32 - '۰' as u32) as u8 + b'0') as char,
            '٠'..='٩' => ((c as u32 - '٠' as u32) as u8 + b'0') as char,
            other => other,
        })
        .collect()
}

/// Jalali date/time in Tehran timezone with English digits via chrono + gregorian_to_jalali.
pub(crate) fn fmt_jalali_dt(epoch: i64) -> String {
    let Some(utc) = chrono::DateTime::from_timestamp(epoch, 0) else {
        return "—".to_string();
    };
    let dt = utc.with_timezone(&Tehran);
    let (jy, jm, jd) = gregorian_to_jalali(dt.year(), dt.month() as i32, dt.day() as i32);
    format!(
        "🗓 {jy}/{jm:02}/{jd:02} ⏰ {:02}:{:02}",
        dt.hour(),
        dt.minute()
    )
}
