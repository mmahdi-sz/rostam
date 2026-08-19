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

    // ── Phase 1: Guaranteed Subtitle Acquisition (NLLB Fallback for 429 / Missing Subs) ──
    ensure_translated_subtitles(
        api,
        cookie_spec,
        webpage_url,
        chat_id,
        msg_id,
        dir,
        &selection.subtitle_langs,
        trace_id,
    )
    .await;

    // ── Phase 2: Subtitle Mode Dispatch ──
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
                Ok(_) => SubtitlePipelineResult::None,
                Err(e) => {
                    log_trace(trace_id, "hardsub_pipeline_error", &e);
                    let _ = edit_status(
                        api,
                        chat_id,
                        msg_id,
                        crate::i18n::t("youtube.download.hardsub_failed"),
                    )
                    .await;
                    SubtitlePipelineResult::None
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
                    SubtitlePipelineResult::None
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
    let mut target_langs = target_langs.to_vec();
    if !target_langs.contains(&"en".to_string()) {
        target_langs.push("en".to_string());
    }
    let sub_langs = target_langs.join(",");

    let mut cmd = tokio::process::Command::new("yt-dlp");
    cmd.arg("--js-runtimes")
        .arg(format!("deno:{}", crate::config::deno_path()))
        .arg("--cookies-from-browser")
        .arg(cookie_spec)
        .arg("--extractor-args")
        .arg("youtubetab:skip=authcheck")
        .arg("--no-warnings")
        .arg("--no-playlist")
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
    let _ = cmd.output().await;
}
