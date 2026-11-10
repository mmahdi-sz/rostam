use std::path::{Path, PathBuf};

use frankenstein::{
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::SendDocumentParams,
};

use crate::bot::AsyncTelegramApiMetered;
use crate::i18n::tf;
use crate::youtube::trace::log_trace;

fn cue_times(timestamp: &str) -> Option<(u64, u64)> {
    fn millis(value: &str) -> Option<u64> {
        let value = value.split_whitespace().next()?;
        let mut parts = value.split([':', ',', '.']);
        let h: u64 = parts.next()?.parse().ok()?;
        let m: u64 = parts.next()?.parse().ok()?;
        let s: u64 = parts.next()?.parse().ok()?;
        let fraction = parts.next()?;
        let ms: u64 = format!("{fraction:0<3}").get(..3)?.parse().ok()?;
        if m >= 60 || s >= 60 {
            return None;
        }
        h.checked_mul(60)?
            .checked_add(m)?
            .checked_mul(60)?
            .checked_add(s)?
            .checked_mul(1000)?
            .checked_add(ms)
    }
    let (start, end) = timestamp.split_once("-->")?;
    Some((millis(start.trim())?, millis(end.trim())?))
}

/// Remove YouTube roll-up carryover before translation, without joining across pauses.
pub fn clean_rolling_subtitles(content: &str) -> String {
    use crate::youtube::translator::{format_srt, parse_srt};
    let items = parse_srt(content);
    let rolling = items.windows(2).any(|pair| {
        let previous: Vec<_> = pair[0]
            .text
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        let next: Vec<_> = pair[1]
            .text
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        previous.last().is_some()
            && previous.last() == next.first()
            && next.len() > 1
            && matches!((cue_times(&pair[0].timestamp), cue_times(&pair[1].timestamp)),
                (Some((_, end)), Some((start, _))) if start.abs_diff(end) <= 100)
    });
    if !rolling {
        return content.to_string();
    }
    let mut cleaned: Vec<crate::youtube::translator::SrtItem> = Vec::new();
    let mut previous_lines: Vec<String> = Vec::new();
    let mut previous_end = 0;
    for mut item in items {
        let Some((start, end)) = cue_times(&item.timestamp) else {
            continue;
        };
        if end <= start {
            continue;
        }
        let lines: Vec<String> = item
            .text
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        // YouTube's 10ms animation cues only repeat the last displayed line.
        if end - start <= 20
            && lines.len() == 1
            && previous_lines.last() == lines.first()
            && start.abs_diff(previous_end) <= 100
        {
            continue;
        }
        let overlap = if start.abs_diff(previous_end) <= 100 && lines.len() > 1 {
            (1..=previous_lines.len().min(lines.len()))
                .rev()
                .find(|&n| previous_lines[previous_lines.len() - n..] == lines[..n])
                .unwrap_or(0)
        } else {
            0
        };
        previous_lines = lines.clone();
        previous_end = end;
        item.text = lines[overlap..].join("\n");
        if item.text.is_empty() {
            continue;
        }
        item.index = cleaned.len() + 1;
        cleaned.push(item);
    }
    format_srt(&cleaned)
}

/// Render simultaneous captions as one chronological block so newer text stays below.
pub fn stack_subtitles_chronologically(content: &str) -> String {
    let items = crate::youtube::translator::parse_srt(content);
    let timed: Vec<_> = items
        .iter()
        .filter_map(|item| {
            cue_times(&item.timestamp)
                .filter(|(start, end)| start < end)
                .map(|(start, end)| (start, end, item))
        })
        .collect();
    let mut boundaries: Vec<_> = timed
        .iter()
        .flat_map(|(start, end, _)| [*start, *end])
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    let stamp = |ms: u64| {
        format!(
            "{:02}:{:02}:{:02},{:03}",
            ms / 3600000,
            ms / 60000 % 60,
            ms / 1000 % 60,
            ms % 1000
        )
    };
    let mut output = Vec::new();
    for window in boundaries.windows(2) {
        let (start, end) = (window[0], window[1]);
        let mut active: Vec<_> = timed
            .iter()
            .filter(|(a, b, _)| *a <= start && *b > start)
            .collect();
        active.sort_by_key(|(a, _, item)| (*a, item.index));
        if active.is_empty() {
            continue;
        }
        output.push(crate::youtube::translator::SrtItem {
            index: output.len() + 1,
            timestamp: format!("{} --> {}", stamp(start), stamp(end)),
            text: active
                .iter()
                .map(|(_, _, item)| item.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        });
    }
    crate::youtube::translator::format_srt(&output)
}

/// Set Persian paragraph direction while preserving Latin words and numbers.
pub fn prepare_subtitle(content: &str, lang: &str) -> String {
    let cleaned = clean_rolling_subtitles(content);
    if lang != "fa" && !lang.starts_with("fa-") {
        return cleaned;
    }
    let mut items = crate::youtube::translator::parse_srt(&cleaned);
    for item in &mut items {
        item.text = item
            .text
            .lines()
            .map(|line| {
                if line.starts_with('\u{202b}') && line.ends_with('\u{202c}') {
                    line.to_string()
                } else {
                    format!("\u{202b}{line}\u{202c}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    crate::youtube::translator::format_srt(&items)
}

pub fn clean_subtitles_in_dir(dir: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("srt") {
            continue;
        }
        let Some(lang) = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(subtitle_lang_of)
        else {
            continue;
        };
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let prepared = prepare_subtitle(&content, &lang);
        if content != prepared {
            std::fs::write(&path, prepared).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Extracts the language tag a subtitle filename was written with, e.g.
/// "video.fa.srt" -> "fa", "sub.en.srt" -> "en", "translated_fa.srt" -> "fa".
pub fn subtitle_lang_of(fname: &str) -> Option<String> {
    let lower = fname.to_lowercase();
    if let Some(rest) = lower.strip_prefix("translated_") {
        return rest.strip_suffix(".srt").map(|s| s.to_string());
    }
    let stem = lower
        .strip_suffix(".srt")
        .or_else(|| lower.strip_suffix(".vtt"))?;
    stem.rsplit('.').next().map(|s| s.to_string())
}

/// Whether `fname` (a subtitle file) belongs to one of the user-selected
/// `target_langs` — used to keep translation-source files (e.g. an English
/// srt fetched only so it can be translated) out of what actually gets
/// delivered/embedded when the user never asked for that language.
pub fn subtitle_matches_selection(fname: &str, target_langs: &[String]) -> bool {
    let Some(lang) = subtitle_lang_of(fname) else {
        return false;
    };
    target_langs.iter().any(|t| {
        let t = t.to_lowercase();
        lang == t || lang.starts_with(&format!("{t}-"))
    })
}

/// True if `dir` already contains a subtitle file (from the main yt-dlp run
/// or a prior pass) for `lang` — used to avoid re-downloading/re-embedding
/// a language that's already on disk.
pub fn subtitle_file_exists_for_lang(dir: &std::path::Path, lang: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let lang = lang.to_lowercase();
    entries.flatten().any(|e| {
        let name = e.file_name().to_string_lossy().to_string();
        e.path().extension().and_then(|ext| ext.to_str()) == Some("srt")
            && subtitle_matches_selection(&name, std::slice::from_ref(&lang))
            && std::fs::read_to_string(e.path())
                .map(|text| {
                    crate::youtube::translator::parse_srt(&text)
                        .iter()
                        .any(|cue| cue.timestamp.contains("-->") && !cue.text.is_empty())
                })
                .unwrap_or(false)
    })
}

/// Finds subtitle files (.srt/.vtt) produced by yt-dlp in `dir` and sends each
/// to the user as a document. Used in SubtitleMode::File. Returns how many were sent.
pub async fn send_subtitle_files(
    api: &Bot,
    dir: &std::path::Path,
    chat_id: i64,
    video_title: &str,
    target_langs: &[String],
    trace_id: u64,
) -> usize {
    let mut sent = 0usize;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut subs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let e = e.to_ascii_lowercase();
                    e == "srt" || e == "vtt"
                })
                .unwrap_or(false)
        })
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| subtitle_matches_selection(n, target_langs))
                .unwrap_or(false)
        })
        .collect();
    subs.sort();
    for sub_path in &subs {
        let fname = sub_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("subtitle");
        // Try to surface the language tag in the caption (e.g. "video.fa.srt" -> "fa").
        let lang = sub_path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.rsplit('.').next())
            .unwrap_or("")
            .to_string();
        let caption = tf(
            "youtube.download.subtitle_caption",
            &[("title", video_title), ("lang", &lang)],
        );
        let params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(FileUpload::InputFile(InputFile {
                path: sub_path.clone(),
            }))
            .caption(caption)
            .build();
        match api.send_document_metered(&params).await {
            Ok(_) => {
                sent += 1;
                log_trace(
                    trace_id,
                    "subtitle_file_sent",
                    &format!("file={fname} lang={lang}"),
                );
            }
            Err(e) => log_trace(
                trace_id,
                "subtitle_file_failed",
                &format!("file={fname} err={e}"),
            ),
        }
    }
    log_trace(
        trace_id,
        "subtitle_files_done",
        &format!("sent={sent} found={}", subs.len()),
    );
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_captions_keep_old_above_new() {
        let raw =
            "1\n00:00:00,000 --> 00:00:04,000\nOld\n\n2\n00:00:02,000 --> 00:00:05,000\nNew\n\n";
        let result = stack_subtitles_chronologically(raw);
        let cues = crate::youtube::translator::parse_srt(&result);
        assert_eq!(cues.len(), 3);
        assert_eq!(cues[0].text, "Old");
        assert_eq!(cues[1].timestamp, "00:00:02,000 --> 00:00:04,000");
        assert_eq!(cues[1].text, "Old\nNew");
        assert_eq!(cues[2].text, "New");
        assert_eq!(stack_subtitles_chronologically(&result), result);
    }

    #[test]
    fn real_youtube_rollup_preserves_words_without_carryover() {
        let raw = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/youtube_rolling.srt"
        ));
        let result = clean_rolling_subtitles(raw);
        let cues = crate::youtube::translator::parse_srt(&result);
        assert_eq!(cues.len(), 7);
        assert_eq!(cues[0].text, "Now we need to talk about what comes");
        assert_eq!(cues[1].text, "next, cuz obviously the Democrats just");
        assert_eq!(cues[6].text, "They have a significant Senate majority.");
        assert_eq!(cues[1].timestamp, "00:00:01,440 --> 00:00:04,390");
        assert_eq!(clean_rolling_subtitles(&result), result);
    }

    #[test]
    fn ordinary_short_cues_numbers_and_repetitions_survive() {
        let raw = "1\n00:00:00,000 --> 00:00:00,050\n123\n\n2\n00:00:02,000 --> 00:00:03,000\nHello\n\n3\n00:00:06,000 --> 00:00:07,000\nHello\nWorld\n\n";
        assert_eq!(clean_rolling_subtitles(raw), raw);
    }

    #[test]
    fn persian_direction_is_idempotent_and_keeps_latin_and_times() {
        let raw = "1\n00:00:00,000 --> 00:00:02,000\nYouTube نسخه 2026: سلام!\nخط دوم\n\n";
        let prepared = prepare_subtitle(raw, "fa");
        let cues = crate::youtube::translator::parse_srt(&prepared);
        assert_eq!(cues[0].timestamp, "00:00:00,000 --> 00:00:02,000");
        assert_eq!(
            cues[0].text,
            "\u{202b}YouTube نسخه 2026: سلام!\u{202c}\n\u{202b}خط دوم\u{202c}"
        );
        assert_eq!(prepare_subtitle(&prepared, "fa"), prepared);
        assert_eq!(prepare_subtitle(raw, "en"), raw);
    }

    #[test]
    fn subtitle_source_accepts_original_english_but_rejects_vtt_and_empty_files() {
        let dir = crate::common::dir::TempDirGuard::create(
            "yt_subtitle_regression",
            crate::log::next_trace_id(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("sub.en-orig.vtt"),
            "WEBVTT\n00:00:00.000 --> 00:00:01.000\nHello",
        )
        .unwrap();
        assert!(!subtitle_file_exists_for_lang(dir.path(), "en"));
        std::fs::write(dir.path().join("sub.en-orig.srt"), "").unwrap();
        assert!(!subtitle_file_exists_for_lang(dir.path(), "en"));
        std::fs::write(
            dir.path().join("sub.en-orig.srt"),
            "1\n00:00:00,000 --> 00:00:01,000\nHello\n",
        )
        .unwrap();
        assert!(subtitle_file_exists_for_lang(dir.path(), "en"));
        assert!(!subtitle_file_exists_for_lang(dir.path(), "fa"));
        assert!(!subtitle_file_exists_for_lang(dir.path(), "en-US"));
    }
}
