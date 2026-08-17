use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{DeleteMessageParams, EditMessageTextParams, SendAudioParams, SendMessageParams},
    types::ReplyMarkup,
};
use tokio::process::Command;

use crate::bot::{audio_separation_keyboard, sp_cancel_keyboard};
use crate::database::postgresql::PostgresDatabase;
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::moebius::cpu::{acquire_cpu, release_cpu};
use crate::spotify::cancel::{SpotifyUnregisterGuard, register_spotify_cancel};
use crate::spotify::client::fetch_spotify_track;
use crate::spotify::search::find_best_youtube_match;
use crate::spotify::tagging::apply_id3_tags;

struct SpotifyWorkDirGuard {
    dir: PathBuf,
    trace_id: u64,
}

impl Drop for SpotifyWorkDirGuard {
    fn drop(&mut self) {
        let dir = self.dir.clone();
        let trace_id = self.trace_id;
        tokio::spawn(async move {
            if dir.exists() {
                if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
                    log_ev!("sp", trace_id, "dir_cleanup_err", "err" => e.to_string());
                } else {
                    log_ev!("sp", trace_id, "dir_cleanup_ok", "path" => dir.display().to_string());
                }
            }
        });
    }
}

pub async fn handle_spotify_url(
    api: &Bot,
    chat_id: i64,
    _trigger_msg_id: i32,
    user_id: i64,
    trace_id: u64,
    track_id: &str,
    database: &Option<PostgresDatabase>,
) -> anyhow::Result<()> {
    log_actor_id!("sp", trace_id, user_id, "track_id" => track_id);

    let stop_flag = register_spotify_cancel(user_id);
    let _cancel_guard = SpotifyUnregisterGuard(user_id);

    let root_dir = PathBuf::from(crate::config::spotify_download_root());
    let job_dir = root_dir.join(format!("job_{}_{trace_id}", user_id.abs()));
    tokio::fs::create_dir_all(&job_dir)
        .await
        .context("Failed creating Spotify work directory")?;
    let _dir_guard = SpotifyWorkDirGuard {
        dir: job_dir.clone(),
        trace_id,
    };

    // Send initial ticker message
    let start_text = apply_premium_to_md(&t("spotify.starting"));
    let mut status_msg_id = 0i32;
    match api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(start_text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(sp_cancel_keyboard()))
                .build(),
        )
        .await
    {
        Ok(res) => {
            status_msg_id = res.result.message_id;
        }
        Err(e) => {
            log_ev!("sp", trace_id, "send_status_failed", "err" => e.to_string());
        }
    }

    let edit_status = |text_md: String| {
        let api_c = api.clone();
        let text_md = apply_premium_to_md(&text_md);
        async move {
            if status_msg_id > 0 {
                let _ = api_c
                    .edit_message_text(
                        &EditMessageTextParams::builder()
                            .chat_id(chat_id)
                            .message_id(status_msg_id)
                            .text(text_md)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(sp_cancel_keyboard())
                            .build(),
                    )
                    .await;
            }
        }
    };

    // Helper for exit on error
    let handle_error = |err_msg_key: &str| {
        let api_c = api.clone();
        let err_text = apply_premium_to_md(&t(err_msg_key));
        async move {
            if status_msg_id > 0 {
                let _ = api_c
                    .edit_message_text(
                        &EditMessageTextParams::builder()
                            .chat_id(chat_id)
                            .message_id(status_msg_id)
                            .text(err_text)
                            .parse_mode(ParseMode::MarkdownV2)
                            .build(),
                    )
                    .await;
            }
        }
    };

    if stop_flag.load(Ordering::SeqCst) {
        log_ev!("sp", trace_id, "job_cancelled_early", "=>" => "cancel");
        handle_error("spotify.cancelled").await;
        return Ok(());
    }

    // ── STAGE 1: Fetch Spotify Metadata ─────────────────────────────────────
    log_ev!("sp", trace_id, "fetch_metadata_enter", "track_id" => track_id);
    edit_status(t("spotify.fetching_metadata")).await;

    let meta = match fetch_spotify_track(track_id).await {
        Ok(m) => {
            log_ev!(
                "sp",
                trace_id,
                "fetch_metadata_ok",
                "title" => &m.title,
                "artist" => &m.artists_joined
            );
            m
        }
        Err(e) => {
            log_ev!("sp", trace_id, "fetch_metadata_fail", "err" => e.to_string());
            crate::stats::record_error_global("spotify", format!("metadata_fetch: {e}")).await;
            handle_error("spotify.track_not_found").await;
            return Ok(());
        }
    };

    if stop_flag.load(Ordering::SeqCst) {
        log_ev!("sp", trace_id, "job_cancelled_stage1", "=>" => "cancel");
        handle_error("spotify.cancelled").await;
        return Ok(());
    }

    // ── STAGE 2: Search YouTube Match ────────────────────────────────────────
    log_ev!("sp", trace_id, "search_youtube_enter", "query" => format!("{} - {}", meta.primary_artist, meta.title));
    let search_msg = tf(
        "spotify.searching_youtube",
        &[
            ("title", &md_escape(&meta.title)),
            ("artist", &md_escape(&meta.primary_artist)),
        ],
    );
    edit_status(search_msg).await;

    let match_cand = match find_best_youtube_match(
        &meta.primary_artist,
        &meta.title,
        meta.duration_ms,
        trace_id,
    )
    .await
    {
        Ok(c) => {
            log_ev!(
                "sp",
                trace_id,
                "search_youtube_ok",
                "url" => &c.webpage_url,
                "score" => c.score
            );
            c
        }
        Err(e) => {
            log_ev!("sp", trace_id, "search_youtube_fail", "err" => e.to_string());
            crate::stats::record_error_global("spotify", format!("youtube_search: {e}")).await;
            handle_error("spotify.no_yt_match").await;
            return Ok(());
        }
    };

    if stop_flag.load(Ordering::SeqCst) {
        log_ev!("sp", trace_id, "job_cancelled_stage2", "=>" => "cancel");
        handle_error("spotify.cancelled").await;
        return Ok(());
    }

    // ── STAGE 3: Download & Transcode Audio via CPU Broker ───────────────────
    log_ev!("sp", trace_id, "download_audio_enter", "url" => &match_cand.webpage_url);
    let dl_msg = tf(
        "spotify.downloading",
        &[
            ("title", &md_escape(&meta.title)),
            ("artist", &md_escape(&meta.artists_joined)),
        ],
    );
    edit_status(dl_msg).await;

    let cores = acquire_cpu(user_id, trace_id).await;
    let mp3_path = job_dir.join("track.mp3");
    let dl_ok = run_yt_dlp_audio(
        &job_dir,
        "track",
        &match_cand.webpage_url,
        &cores,
        trace_id,
        &stop_flag,
    )
    .await;
    release_cpu(cores, trace_id).await;

    match dl_ok {
        DlOutcome::Ok => {}
        DlOutcome::Cancelled => {
            log_ev!("sp", trace_id, "job_cancelled_during_dl", "=>" => "cancel");
            handle_error("spotify.cancelled").await;
            return Ok(());
        }
        DlOutcome::Failed => {
            crate::stats::record_error_global("spotify", "yt_dlp audio download failed").await;
            handle_error("spotify.download_failed").await;
            return Ok(());
        }
    }

    if !mp3_path.exists() {
        log_ev!("sp", trace_id, "mp3_not_found", "path" => mp3_path.display().to_string());
        crate::stats::record_error_global("spotify", "mp3 file missing after yt-dlp").await;
        handle_error("spotify.download_failed").await;
        return Ok(());
    }

    if stop_flag.load(Ordering::SeqCst) {
        log_ev!("sp", trace_id, "job_cancelled_stage3", "=>" => "cancel");
        handle_error("spotify.cancelled").await;
        return Ok(());
    }

    // ── STAGE 4: Embed ID3 Tags & Cover Art ──────────────────────────────────
    log_ev!("sp", trace_id, "tagging_enter", "path" => mp3_path.display().to_string());
    edit_status(t("spotify.tagging")).await;

    let cover_file_path = apply_id3_tags(&mp3_path, &meta, "cover", trace_id)
        .await
        .unwrap_or(None);

    if stop_flag.load(Ordering::SeqCst) {
        log_ev!("sp", trace_id, "job_cancelled_stage4", "=>" => "cancel");
        handle_error("spotify.cancelled").await;
        return Ok(());
    }

    // ── STAGE 5: Upload Audio Message ────────────────────────────────────────
    log_ev!("sp", trace_id, "upload_enter", "file" => mp3_path.display().to_string());
    edit_status(t("spotify.uploading")).await;

    let duration_secs = (meta.duration_ms + 500) / 1000;

    let is_fa = crate::i18n::current_lang() == "fa";
    let raw_date = meta.release_date.as_deref().unwrap_or("");
    let date_str = if raw_date.is_empty() {
        "-".to_string()
    } else {
        format_spotify_release_date(raw_date, is_fa)
    };

    let caption = tf(
        "spotify.done_caption",
        &[
            ("title", &md_escape(&meta.title)),
            ("artist", &md_escape(&meta.artists_joined)),
            ("album", &md_escape(&meta.album_name)),
            ("date", &md_escape(&date_str)),
        ],
    );

    let mut send_audio_params = SendAudioParams::builder()
        .chat_id(chat_id)
        .audio(FileUpload::InputFile(InputFile {
            path: mp3_path.clone(),
        }))
        .performer(meta.artists_joined.clone())
        .title(meta.title.clone())
        .duration(duration_secs as u32)
        .caption(apply_premium_to_md(&caption))
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(
            audio_separation_keyboard(),
        ))
        .build();

    if let Some(cover_path) = cover_file_path {
        send_audio_params.thumbnail = Some(FileUpload::InputFile(InputFile { path: cover_path }));
    }

    let file_size = tokio::fs::metadata(&mp3_path)
        .await
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    let up_start = std::time::Instant::now();
    let stats_job_id = crate::stats::record_download_start(user_id, "spotify").await;

    use crate::bot::send_file_with_upload_ticker;
    let upload_res = send_file_with_upload_ticker::<_, frankenstein::types::Message>(
        api,
        "sendAudio",
        &send_audio_params,
        &mp3_path,
        chat_id,
        status_msg_id,
        "transfer.stage.sending_audio",
        None,
    )
    .await;

    match upload_res {
        Ok(_) => {
            log_ev!("sp", trace_id, "upload_ok", "=>" => "ok");

            let up_elapsed = up_start.elapsed();
            let up_speed = if up_elapsed.as_secs_f64() > 0.0 {
                file_size as f64 / up_elapsed.as_secs_f64()
            } else {
                0.0
            };

            if let Some(jid) = stats_job_id {
                crate::stats::record_upload_done(
                    jid,
                    user_id,
                    file_size,
                    Some(up_speed as i64),
                    Some(1),
                )
                .await;
            }

            // Record stats & quota
            crate::stats::record_event_user(user_id, "spotify", "download", "ok", 1).await;
            crate::stats::record_event_global("spotify", "download", "ok", 1).await;

            if let Some(db) = database {
                if let Some(first_up) =
                    crate::rank::quota::get_first_upload_at(db.client(), user_id).await
                {
                    let _ =
                        crate::rank::quota::add_traffic(db.client(), user_id, file_size, first_up)
                            .await;
                }
            }

            // Clean status message after successful upload
            if status_msg_id > 0 {
                let _ = api
                    .delete_message(
                        &DeleteMessageParams::builder()
                            .chat_id(chat_id)
                            .message_id(status_msg_id)
                            .build(),
                    )
                    .await;
            }
        }
        Err(e) => {
            log_ev!("sp", trace_id, "upload_fail", "err" => e.to_string());
            crate::stats::record_error_global("spotify", format!("upload_audio: {e}")).await;
            handle_error("spotify.download_failed").await;
        }
    }

    Ok(())
}

pub enum DlOutcome {
    Ok,
    Cancelled,
    Failed,
}

/// Run yt-dlp for one track into `job_dir/{stem}.mp3`, killing it on cancel.
///
/// Cores are acquired/released by the caller — the playlist runner holds one
/// reservation across a whole batch instead of re-queuing per track.
pub async fn run_yt_dlp_audio(
    job_dir: &std::path::Path,
    stem: &str,
    webpage_url: &str,
    cores: &[i32],
    trace_id: u64,
    stop_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> DlOutcome {
    let out_template = job_dir.join(format!("{stem}.%(ext)s"));
    let _ = cores; // pinning is handled by the broker; kept for call-site clarity

    let mut child = match Command::new("yt-dlp")
        .arg("--js-runtimes")
        .arg(format!("deno:{}", crate::config::deno_path()))
        .arg("-x")
        .arg("--audio-format")
        .arg("mp3")
        .arg("--audio-quality")
        .arg("320k")
        .arg("--no-warnings")
        .arg("-o")
        .arg(&out_template)
        .arg(webpage_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log_ev!("sp", trace_id, "yt_dlp_spawn_fail", "err" => e.to_string());
            return DlOutcome::Failed;
        }
    };

    let timeout_secs = 300u64;
    let start_instant = std::time::Instant::now();

    loop {
        if stop_flag.load(Ordering::SeqCst) {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return DlOutcome::Cancelled;
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return DlOutcome::Ok;
                }
                log_ev!("sp", trace_id, "yt_dlp_exit_fail", "status" => status.to_string());
                return DlOutcome::Failed;
            }
            Ok(None) => {
                if start_instant.elapsed() > Duration::from_secs(timeout_secs) {
                    let _ = child.start_kill();
                    log_ev!("sp", trace_id, "yt_dlp_timeout", "=>" => "timeout");
                    return DlOutcome::Failed;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(e) => {
                log_ev!("sp", trace_id, "yt_dlp_wait_err", "err" => e.to_string());
                return DlOutcome::Failed;
            }
        }
    }
}

pub fn format_spotify_release_date(raw_date: &str, is_fa: bool) -> String {
    let clean = raw_date.split('T').next().unwrap_or(raw_date);
    let parts: Vec<&str> = clean.split('-').collect();
    if parts.len() == 3 {
        let Ok(gy) = parts[0].parse::<i32>() else {
            return raw_date.to_string();
        };
        let Ok(gm) = parts[1].parse::<i32>() else {
            return raw_date.to_string();
        };
        let Ok(gd) = parts[2].parse::<i32>() else {
            return raw_date.to_string();
        };

        if is_fa {
            let (jy, jm, jd) = crate::youtube::jalali::gregorian_to_jalali(gy, gm, gd);
            format!("{jy:04}/{jm:02}/{jd:02}")
        } else {
            format!("{gy:04}-{gm:02}-{gd:02}")
        }
    } else {
        raw_date.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_spotify_release_date_shamsi() {
        assert_eq!(
            format_spotify_release_date("2025-01-17T00:00:00Z", true),
            "1403/10/28"
        );
    }

    #[test]
    fn test_format_spotify_release_date_gregorian() {
        assert_eq!(
            format_spotify_release_date("2025-01-17T00:00:00Z", false),
            "2025-01-17"
        );
    }
}
