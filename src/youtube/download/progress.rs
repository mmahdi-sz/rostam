use crate::i18n::tf;

#[derive(Default, Clone)]
pub struct ProgressSnapshot {
    pub percent: String,
    pub downloaded: String,
    pub total: String,
    pub speed: String,
    pub eta: String,
    pub elapsed: String,
    pub percent_int: i32,
}

const PROGRESS_PREFIX: &str = "YT_PROGRESS|";

pub fn parse_progress_line(line: &str) -> Option<ProgressSnapshot> {
    let rest = line.strip_prefix(PROGRESS_PREFIX)?;
    let parts: Vec<&str> = rest.split('|').collect();
    if parts.len() < 7 {
        return None;
    }
    let percent_str = parts[0].trim().to_string();
    let percent_int = percent_str
        .trim_end_matches('%')
        .trim()
        .parse::<f32>()
        .ok()
        .map(|f| f.round() as i32)
        .unwrap_or(-1);
    let total = {
        let exact = parts[2].trim();
        if exact.is_empty() || exact == "N/A" {
            parts[3].trim().to_string()
        } else {
            exact.to_string()
        }
    };
    Some(ProgressSnapshot {
        percent: percent_str,
        downloaded: parts[1].trim().to_string(),
        total,
        speed: parts[4].trim().to_string(),
        eta: parts[5].trim().to_string(),
        elapsed: parts[6].trim().to_string(),
        percent_int,
    })
}

pub fn build_bar(percent: f32) -> String {
    let total = 10usize;
    let filled = ((percent / 10.0).round() as i32).clamp(0, total as i32) as usize;
    let mut s = String::new();
    for _ in 0..filled { s.push('●'); }
    for _ in 0..(total - filled) { s.push('○'); }
    s
}

fn clean_val<'a>(s: &'a str, fallback: &'a str) -> &'a str {
    let s = s.trim();
    if s.is_empty() || s == "N/A" || s == "?" || s.starts_with("Unknown") {
        fallback
    } else {
        s
    }
}

pub fn format_progress_body(snap: &ProgressSnapshot, quality_label: &str) -> String {
    let percent_f = snap
        .percent
        .trim_end_matches('%')
        .trim()
        .parse::<f32>()
        .unwrap_or(0.0);
    let bar = build_bar(percent_f);
    let percent = clean_val(&snap.percent, "۰٪");
    let downloaded = clean_val(&snap.downloaded, "-");
    let total = clean_val(&snap.total, "-");
    let speed = clean_val(&snap.speed, "...");
    let eta = clean_val(&snap.eta, "...");
    let elapsed = clean_val(&snap.elapsed, "۰۰:۰۰");
    tf(
        "youtube.download.progress.body",
        &[
            ("quality", quality_label),
            ("percent", percent),
            ("bar", &bar),
            ("downloaded", downloaded),
            ("total", total),
            ("speed", speed),
            ("elapsed", elapsed),
            ("eta", eta),
        ],
    )
}

pub fn format_elapsed(d: std::time::Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

pub fn format_upload_body(quality_label: &str, elapsed: std::time::Duration) -> String {
    tf(
        "youtube.download.progress.upload_body",
        &[
            ("quality", quality_label),
            ("elapsed", &format_elapsed(elapsed)),
        ],
    )
}
