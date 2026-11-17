use frankenstein::client_reqwest::Bot;

use crate::common::cpu_broker::CpuBrokerGuard;
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
    source_hint: Option<&str>,
    user_id: i64,
) -> Result<(), String> {
    if target_langs.is_empty() {
        return Ok(());
    }

    super::files::clean_subtitles_in_dir(dir)?;
    let Some((source_srt, source_lang)) = find_source_subtitle(dir, source_hint) else {
        return Err("no supported source subtitle".to_string());
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
        if subtitle_file_exists_for_lang(dir, tgt) {
            continue;
        }

        let Some(nllb_lang) = crate::youtube::translator::map_language_code(tgt) else {
            continue;
        };

        let out_path = dir.join(format!("translated_{tgt}.srt"));

        if CpuBrokerGuard::is_user_busy(user_id).await {
            return Err("active translation job running".to_string());
        }
        let mut broker = CpuBrokerGuard::acquire(user_id, trace_id, "yt").await;
        if !broker.is_acquired() {
            return Err("CPU broker unavailable".to_string());
        }
        let num_threads = broker.cores().len().max(1);

        let progress_api = api.clone();
        let mut last_edit = std::time::Instant::now() - std::time::Duration::from_secs(2);
        let mut last_percent_int: i32 = -1;

        let res = crate::youtube::translator::translate_srt(
            &source_srt,
            &out_path,
            source_lang,
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

        broker.release().await;

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

pub(super) fn find_source_subtitle(
    dir: &std::path::Path,
    source_hint: Option<&str>,
) -> Option<(std::path::PathBuf, &'static str)> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let lang = super::files::subtitle_lang_of(name)?;
            let original = lang.ends_with("-orig");
            let hinted = source_hint == Some(lang.as_str());
            if (!original && !hinted && lang != "en")
                || name.starts_with("translated_")
                || !name.ends_with(".srt")
            {
                return None;
            }
            let code = crate::youtube::translator::map_language_code(&lang)?;
            let text = std::fs::read_to_string(&path).ok()?;
            crate::youtube::translator::parse_srt(&text)
                .iter()
                .any(|cue| cue.timestamp.contains("-->") && !cue.text.is_empty())
                .then_some((!original, !hinted, path, code))
        })
        .min()
        .map(|(_, _, path, code)| (path, code))
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
    source_hint: Option<&str>,
    trace_id: u64,
    user_id: i64,
) {
    let missing: Vec<String> = target_langs
        .iter()
        .filter(|lang| !subtitle_file_exists_for_lang(dir, lang))
        .cloned()
        .collect();
    if missing.is_empty() {
        return;
    }
    super::download_subtitles_separately(cookie_spec, webpage_url, dir, &missing, trace_id).await;
    let missing_translatable: Vec<String> = missing
        .into_iter()
        .filter(|lang| !subtitle_file_exists_for_lang(dir, lang))
        .filter(|lang| crate::youtube::translator::map_language_code(lang).is_some())
        .collect();
    if missing_translatable.is_empty() {
        return;
    }
    if find_source_subtitle(dir, source_hint).is_none() {
        super::download_subtitles_separately(
            cookie_spec,
            webpage_url,
            dir,
            &[".*-orig".to_string()],
            trace_id,
        )
        .await;
    }
    if find_source_subtitle(dir, source_hint).is_none()
        && let Some(lang) =
            source_hint.filter(|lang| crate::youtube::translator::map_language_code(lang).is_some())
        && !missing_translatable.iter().any(|missing| missing == lang)
    {
        super::download_subtitles_separately(
            cookie_spec,
            webpage_url,
            dir,
            &[lang.to_owned()],
            trace_id,
        )
        .await;
    }
    if find_source_subtitle(dir, source_hint).is_none()
        && !missing_translatable.iter().any(|lang| lang == "en")
    {
        super::download_subtitles_separately(
            cookie_spec,
            webpage_url,
            dir,
            &["en".to_string()],
            trace_id,
        )
        .await;
    }
    if let Err(e) = translate_subtitles(
        api,
        chat_id,
        msg_id,
        trace_id,
        dir,
        &missing_translatable,
        source_hint,
        user_id,
    )
    .await
    {
        log_trace(trace_id, "ensure_translate_failed", &format!("err={e}"));
    }
}
