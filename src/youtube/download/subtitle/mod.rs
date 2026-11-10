pub mod embed;
pub mod files;
pub mod hardsub;
pub mod translation;

pub use embed::*;
pub use files::*;
pub use hardsub::*;
pub use translation::*;

use frankenstein::client_reqwest::Bot;

use crate::youtube::download::status::edit_status;
use crate::youtube::download::types::{Selection, SubtitleMode};
use crate::youtube::trace::log_trace;

pub enum SubtitlePipelineResult {
    VideoUpdated(String),
    SendAsFiles,
    None,
    Failed,
}

pub async fn process_subtitle_pipeline(
    api: &Bot,
    chat_id: i64,
    msg_id: i32,
    dir: &std::path::Path,
    video_path: &str,
    selection: &Selection,
    cookie_spec: &str,
    webpage_url: &str,
    duration_secs: Option<u64>,
    trace_id: u64,
    user_id: i64,
) -> SubtitlePipelineResult {
    if selection.subtitle_langs.is_empty() {
        return SubtitlePipelineResult::None;
    }

    // Retry missing languages independently so a rate limit cannot skip the rest.
    ensure_translated_subtitles(
        api,
        cookie_spec,
        webpage_url,
        chat_id,
        msg_id,
        dir,
        &selection.subtitle_langs,
        trace_id,
        user_id,
    )
    .await;

    let missing: Vec<_> = selection
        .subtitle_langs
        .iter()
        .filter(|lang| !subtitle_file_exists_for_lang(dir, lang))
        .cloned()
        .collect();
    if !missing.is_empty() {
        log_trace(
            trace_id,
            "subtitle_missing",
            &format!("langs={missing:?} => fail"),
        );
        crate::stats::record_error_global("youtube", "selected subtitles unavailable").await;
        edit_status(
            api,
            chat_id,
            msg_id,
            crate::i18n::t("youtube.download.subtitle_failed"),
        )
        .await;
        return SubtitlePipelineResult::Failed;
    }

    // Dispatch only after every selected language is available.
    match selection.subtitle_mode {
        SubtitleMode::Hardsub => {
            match hardsub_subtitles(
                api,
                chat_id,
                msg_id,
                dir,
                video_path,
                &selection.subtitle_langs,
                duration_secs,
                trace_id,
                user_id,
            )
            .await
            {
                Ok(new_path) if new_path != video_path => {
                    SubtitlePipelineResult::VideoUpdated(new_path)
                }
                Ok(_) => SubtitlePipelineResult::Failed,
                Err(e) => {
                    log_trace(trace_id, "hardsub_pipeline_error", &e);
                    crate::stats::record_error_global("youtube", "hardsub failed").await;
                    let _ = edit_status(
                        api,
                        chat_id,
                        msg_id,
                        crate::i18n::t("youtube.download.subtitle_failed"),
                    )
                    .await;
                    SubtitlePipelineResult::Failed
                }
            }
        }
        SubtitleMode::Embedded => {
            match embed_subtitles(dir, video_path, &selection.subtitle_langs, trace_id).await {
                Ok(new_path) if new_path != video_path => {
                    SubtitlePipelineResult::VideoUpdated(new_path)
                }
                Ok(_) => SubtitlePipelineResult::None,
                Err(e) => {
                    log_trace(trace_id, "embed_pipeline_error", &e);
                    crate::stats::record_error_global("youtube", "subtitle embedding failed").await;
                    edit_status(
                        api,
                        chat_id,
                        msg_id,
                        crate::i18n::t("youtube.download.subtitle_failed"),
                    )
                    .await;
                    SubtitlePipelineResult::Failed
                }
            }
        }
        SubtitleMode::File => SubtitlePipelineResult::SendAsFiles,
    }
}

/// Downloads subtitles for `target_langs` (plus English, always, as a
/// translation source/fallback) as standalone .srt files named `sub.<ext>`.
/// Passing an empty slice fetches English only.
pub async fn download_subtitles_separately(
    cookie_spec: &str,
    webpage_url: &str,
    dir: &std::path::Path,
    target_langs: &[String],
    trace_id: u64,
) {
    let mut langs = target_langs.to_vec();
    if !langs.iter().any(|lang| lang == "en") {
        langs.push("en".to_string());
    }
    for lang in langs {
        if subtitle_file_exists_for_lang(dir, &lang) {
            continue;
        }
        let sub_langs = if lang == "en" { "en-orig,en" } else { &lang };

        let mut cmd = tokio::process::Command::new("yt-dlp");
        cmd.kill_on_drop(true)
            .arg("--ignore-config")
            .arg("--js-runtimes")
            .arg(format!("deno:{}", crate::config::deno_path()))
            .arg("--cookies-from-browser")
            .arg(cookie_spec)
            .arg("--extractor-args")
            .arg("youtubetab:skip=authcheck")
            .arg("--no-warnings")
            .arg("--no-playlist")
            .arg("--ignore-errors")
            .arg("--sub-format")
            .arg("srt/vtt/best")
            .arg("--write-subs")
            .arg("--write-auto-subs")
            .arg("--sub-langs")
            .arg(&sub_langs)
            .arg("--convert-subs")
            .arg("srt")
            .arg("--skip-download")
            .arg("-o")
            .arg(format!("{}/sub.%(ext)s", dir.display()))
            .arg(webpage_url);

        log_trace(
            trace_id,
            "download_subtitles_separately_start",
            &format!("langs={sub_langs}"),
        );
        match tokio::time::timeout(std::time::Duration::from_secs(60), cmd.output()).await {
            Ok(Ok(out)) => {
                let available = subtitle_file_exists_for_lang(dir, &lang);
                log_trace(
                    trace_id,
                    "download_subtitles_separately_done",
                    &format!(
                        "lang={lang} status={} available={available} => {}",
                        out.status,
                        if available { "ok" } else { "fail" }
                    ),
                );
                if !out.status.success() || !available {
                    let err = String::from_utf8_lossy(&out.stderr);
                    let tail: String = err
                        .chars()
                        .rev()
                        .take(2000)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                    log_trace(
                        trace_id,
                        "download_subtitles_separately_error",
                        &format!("lang={lang} err={tail}"),
                    );
                }
            }
            Ok(Err(e)) => log_trace(
                trace_id,
                "download_subtitles_separately_error",
                &format!("lang={lang} err={e}"),
            ),
            Err(_) => log_trace(
                trace_id,
                "download_subtitles_separately_timeout",
                &format!("lang={lang} => fail"),
            ),
        }
    }
}
