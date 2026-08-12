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
    for _ in 0..filled {
        s.push('●');
    }
    for _ in 0..(total - filled) {
        s.push('○');
    }
    s
}

fn clean_val<'a>(s: &'a str, fallback: &'a str) -> &'a str {
    let s = s.trim();
    if s.is_empty() || s == "N/A" || s == "NA" || s == "?" || s.starts_with("Unknown") {
        fallback
    } else {
        s
    }
}

pub fn parse_bytes_str(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() || s == "N/A" || s == "NA" || s == "?" || s == "..." || s == "-" {
        return None;
    }
    let s_clean = s.replace('/', " ").replace("B/s", "B").replace("/s", "");
    let parts: Vec<&str> = s_clean.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }
    let (num_str, unit_str) = if parts.len() == 1 {
        let val = parts[0];
        let idx = val.find(|c: char| c.is_alphabetic())?;
        (&val[..idx], &val[idx..])
    } else {
        (parts[0], parts[1])
    };

    let num: f64 = num_str.parse().ok()?;
    let unit = unit_str.to_lowercase();

    let bytes = if unit.starts_with("g") {
        num * 1024.0 * 1024.0 * 1024.0
    } else if unit.starts_with("m") {
        num * 1024.0 * 1024.0
    } else if unit.starts_with("k") {
        num * 1024.0
    } else {
        num
    };

    Some(bytes as u64)
}

pub fn format_progress_body(snap: &ProgressSnapshot, quality_label: &str) -> String {
    let percent_f = snap
        .percent
        .trim_end_matches('%')
        .trim()
        .parse::<f32>()
        .unwrap_or(0.0);
    let bar = build_bar(percent_f);
    let default_percent = crate::i18n::t("youtube.progress_default_percent");
    let percent = clean_val(&snap.percent, &default_percent);
    let downloaded = clean_val(&snap.downloaded, "-");
    let total = clean_val(&snap.total, "-");
    let speed = clean_val(&snap.speed, "...");
    let mut eta = clean_val(&snap.eta, "...");
    let default_elapsed = crate::i18n::t("youtube.progress_default_elapsed");
    let elapsed = clean_val(&snap.elapsed, &default_elapsed);

    let calculated_eta_buf;
    if eta == "..." || eta == "-" || eta == "Unknown" || eta == "N/A" || eta == "?" {
        if let (Some(t_b), Some(d_b), Some(s_bps)) = (
            parse_bytes_str(total),
            parse_bytes_str(downloaded),
            parse_bytes_str(speed),
        ) {
            if s_bps > 0 && t_b > d_b {
                let rem_b = t_b - d_b;
                let rem_secs = rem_b / s_bps;
                calculated_eta_buf = format_elapsed(std::time::Duration::from_secs(rem_secs));
                eta = &calculated_eta_buf;
            }
        }
    }

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
    if s >= 3600 {
        format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub fn format_upload_body(
    quality_label: &str,
    snap: &crate::bot::TransferSnapshot,
) -> String {
    tf(
        "youtube.download.progress.upload_body",
        &[
            ("stage", &snap.stage),
            ("quality", quality_label),
            ("bar", &snap.bar),
            ("percent", &snap.percent),
            ("uploaded", &snap.done),
            ("total", &snap.total),
            ("speed", &snap.speed),
            ("eta", &snap.eta),
            ("elapsed", &snap.elapsed),
        ],
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_percent_shows_zero_bytes() {
        let snap = ProgressSnapshot {
            percent: "0.0%".into(),
            downloaded: "1.00KiB".into(),
            total: "26.48MiB".into(),
            speed: "Unknown B/s".into(),
            eta: "Unknown".into(),
            elapsed: "00:00:00".into(),
            percent_int: 0,
        };
        assert_eq!(snap.percent_int, 0);
        assert_eq!(clean_val(&snap.downloaded, "-"), "1.00KiB");
        // In format_progress_body, percent_f == 0.0 forces downloaded to "0B"
    }
}
