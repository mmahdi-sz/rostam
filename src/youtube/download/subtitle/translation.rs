use frankenstein::client_reqwest::Bot;

use crate::youtube::download::cpu::{acquire_cpu, release_cpu};
use crate::youtube::download::status::edit_status;
use crate::youtube::format::format_duration;
use crate::youtube::trace::log_trace;

use super::files::subtitle_file_exists_for_lang;
use super::hardsub::make_hardsub_bar;

pub async fn translate_subtitles(
    api: &Bot,
    chat_id: i64,
    msg_id: i32,
    trace_id: u64,
    dir: &std::path::Path,
    target_langs: &[String],
) -> Result<(), String> {
    if target_langs.is_empty() {
        return Ok(());
    }

    let mut english_srt = None;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut srts = Vec::new();
    let mut has_target = false;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("srt") {
            let fname = path
                .file_name()
                .unwrap_or_default()
                .to_str()
                .unwrap_or_default()
                .to_string();
            srts.push(path.clone());
            for tgt in target_langs {
                if fname.contains(&format!(".{tgt}."))
                    || fname.contains(&format!(".{tgt}-"))
                    || fname.contains(&format!("_{tgt}.srt"))
                {
                    has_target = true;
                }
            }
            if fname.contains(".en.") || fname.contains(".en-") || fname.contains("_en.srt") {
                english_srt = Some(path.clone());
            }
        }
    }

    if has_target {
        return Ok(());
    }

    let Some(english_srt) = english_srt else {
        return Ok(());
    };

    if msg_id > 0 {
        edit_status(
            api,
            chat_id,
            msg_id,
            crate::i18n::tf("youtube.download.translating_subtitle", &[]),
        )
        .await;
    }

    for tgt in target_langs {
        if tgt == "en" || tgt.starts_with("en-") {
            continue;
        }

        let Some(nllb_lang) = crate::youtube::translator::map_language_code(tgt) else {
            continue;
        };

        let out_path = dir.join(format!("translated_{tgt}.srt"));

        let cores = acquire_cpu(0, trace_id).await;
        let num_threads = if cores.is_empty() { 4 } else { cores.len() };

        let progress_api = api.clone();
        let mut last_edit = std::time::Instant::now() - std::time::Duration::from_secs(2);
        let mut last_percent_int: i32 = -1;

        let res = crate::youtube::translator::translate_srt(
            &english_srt,
            &out_path,
            nllb_lang,
            num_threads,
            move |done, total, elapsed_secs, eta_secs| {
                let percent = if total > 0 { ((done as f64 / total as f64) * 100.0) as u32 } else { 0 };
                let percent_int = percent as i32;
                let elapsed_str = format_duration(elapsed_secs);
                let eta_str = format_duration(eta_secs);

                log_trace(trace_id, "translate_progress", &format!(
                    "done={done} total={total} percent={percent}% elapsed={elapsed_str} eta={eta_str}"
                ));

                let now = std::time::Instant::now();
                if msg_id > 0 && (percent_int != last_percent_int || done == total) && now.duration_since(last_edit) >= std::time::Duration::from_secs(1) {
                    last_percent_int = percent_int;
                    last_edit = now;
                    let bar = make_hardsub_bar(percent);
                    let text = crate::i18n::tf("youtube.download.translating_progress", &[
                        ("bar", &bar),
                        ("percent", &format!("{percent}%")),
                        ("done", &done.to_string()),
                        ("total", &total.to_string()),
                        ("elapsed", &elapsed_str),
                        ("eta", &eta_str),
                    ]);
                    let api_clone = progress_api.clone();
                    tokio::spawn(async move {
                        let _ = edit_status(&api_clone, chat_id, msg_id, text).await;
                    });
                }
            },
        ).await;

        release_cpu(cores, trace_id).await;

        match res {
            Ok(()) => {
                log_trace(trace_id, "translate_subtitle_ok", &format!("lang={tgt}"));
            }
            Err(e) => {
                log_trace(
                    trace_id,
                    "translate_subtitle_error",
                    &format!("lang={tgt} err={e}"),
                );
            }
        }
    }
    Ok(())
}

/// Ensures target subtitles exist by falling back to local translation (NLLB).
pub async fn ensure_translated_subtitles(
    api: &Bot,
    cookie_spec: &str,
    webpage_url: &str,
    chat_id: i64,
    msg_id: i32,
    dir: &std::path::Path,
    target_langs: &[String],
    trace_id: u64,
) {
    let missing_translatable: Vec<String> = target_langs
        .iter()
        .filter(|l| *l != "en" && !l.starts_with("en-"))
        .filter(|l| crate::youtube::translator::map_language_code(l).is_some())
        .filter(|l| !subtitle_file_exists_for_lang(dir, l))
        .cloned()
        .collect();

    if missing_translatable.is_empty() {
        return;
    }

    if !subtitle_file_exists_for_lang(dir, "en") {
        super::download_subtitles_separately(cookie_spec, webpage_url, dir, &[], trace_id).await;
    }

    if let Err(e) =
        translate_subtitles(api, chat_id, msg_id, trace_id, dir, &missing_translatable).await
    {
        log_trace(trace_id, "ensure_translate_failed", &format!("err={e}"));
    }
}
