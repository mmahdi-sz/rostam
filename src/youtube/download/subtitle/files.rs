use std::path::PathBuf;

use frankenstein::{
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::SendDocumentParams,
};

use crate::bot::AsyncTelegramApiMetered;
use crate::i18n::tf;
use crate::youtube::trace::log_trace;

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
        subtitle_lang_of(&name).map(|l| l == lang).unwrap_or(false)
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
