//! Main pipeline for downloading SoundCloud tracks.

use std::path::{Path, PathBuf};
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
use id3::frame::{Picture, PictureType};
use id3::{Tag, TagLike};
use tokio::process::Command;

use crate::bot::{audio_separation_keyboard, sc_cancel_keyboard};
use crate::database::postgresql::PostgresDatabase;
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::moebius::cpu::{acquire_cpu, release_cpu};
use crate::soundcloud::cancel::{SoundcloudUnregisterGuard, register_soundcloud_cancel};
use crate::soundcloud::fetch::{SoundcloudTrackMeta, fetch_soundcloud_meta};
use crate::spotify::handle::format_spotify_release_date;

struct WorkDirGuard(PathBuf);

impl Drop for WorkDirGuard {
    fn drop(&mut self) {
        let path = self.0.clone();
        tokio::spawn(async move {
            let _ = tokio::fs::remove_dir_all(&path).await;
        });
    }
}

pub async fn handle_soundcloud_url(
    api: &Bot,
    chat_id: i64,
    _reply_to_msg_id: i32,
    user_id: i64,
    trace_id: u64,
    sc_url: &str,
    database: &Option<PostgresDatabase>,
) -> anyhow::Result<()> {
    log_actor_id!("sc", trace_id, user_id, "url" => sc_url);

    // Register cancellation flag & RAII guard
    let cancel_flag = register_soundcloud_cancel(user_id);
    let _cancel_guard = SoundcloudUnregisterGuard(user_id);

    // Setup working directory
    let job_dir = std::path::Path::new(&crate::config::soundcloud_download_root())
        .join(format!("job_{user_id}_{trace_id}"));
    tokio::fs::create_dir_all(&job_dir)
        .await
        .context("Failed to create SoundCloud work directory")?;
    let _dir_guard = WorkDirGuard(job_dir.clone());

    // ── STAGE 1: Status Message & Ticker Setup ──────────────────────────────
    let start_text = apply_premium_to_md(&t("soundcloud.starting"));
    let mut status_msg_id = 0;

    let send_params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&start_text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(sc_cancel_keyboard()))
        .build();

    if let Ok(resp) = api.send_message(&send_params).await {
        status_msg_id = resp.result.message_id;
    }

    let edit_status = |text: String| {
        let api = api.clone();
        async move {
            if status_msg_id > 0 {
                let edited = apply_premium_to_md(&text);
                let _ = api
                    .edit_message_text(
                        &EditMessageTextParams::builder()
                            .chat_id(chat_id)
                            .message_id(status_msg_id)
                            .text(&edited)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(sc_cancel_keyboard())
                            .build(),
                    )
                    .await;
            }
        }
    };

    let handle_error = |err_key: &'static str| {
        let api = api.clone();
        async move {
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
            // متن i18n خودش escape شده؛ باید با MarkdownV2 برود
            let _ = crate::bot::send_text_md(&api, chat_id, &t(err_key)).await;
            // re-arm: کاربر بدون منو گیر می‌کرد
            let _ = crate::bot::send_start_menu(&api, chat_id).await;
        }
    };

    if cancel_flag.load(Ordering::Relaxed) {
        handle_error("soundcloud.cancelled").await;
        return Ok(());
    }

    // ── STAGE 2: Fetch Track Metadata ───────────────────────────────────────
    log_ev!("sc", trace_id, "fetch_metadata_enter", "url" => sc_url);
    let meta = match fetch_soundcloud_meta(trace_id, sc_url).await {
        Ok(m) => m,
        Err(e) => {
            log_ev!("sc", trace_id, "fetch_metadata_fail", "err" => e.to_string());
            crate::stats::record_error_global("soundcloud", format!("fetch_meta: {e}")).await;
            // DRM پیام مخصوص دارد، وگرنه کاربر لینکش را بی‌دلیل عوض می‌کند
            let key = if e.to_string().contains("DRM") {
                "soundcloud.drm_protected"
            } else {
                "soundcloud.track_not_found"
            };
            handle_error(key).await;
            return Ok(());
        }
    };

    if cancel_flag.load(Ordering::Relaxed) {
        handle_error("soundcloud.cancelled").await;
        return Ok(());
    }

    // ── STAGE 3: Downloading & Transcoding via yt-dlp ───────────────────────
    log_ev!("sc", trace_id, "download_audio_enter", "title" => &meta.title);
    let dl_status_text = tf(
        "soundcloud.downloading",
        &[
            ("title", &md_escape(&meta.title)),
            ("artist", &md_escape(&meta.artist)),
        ],
    );
    edit_status(dl_status_text).await;

    // Acquire CPU Broker cores for ffmpeg transcode
    let target_mp3 =
        match download_soundcloud_audio(&job_dir, "track", sc_url, user_id, trace_id, &cancel_flag)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                if cancel_flag.load(Ordering::Relaxed) {
                    handle_error("soundcloud.cancelled").await;
                    return Ok(());
                }
                crate::stats::record_error_global("soundcloud", format!("download_audio: {e}"))
                    .await;
                handle_error("soundcloud.download_failed").await;
                return Ok(());
            }
        };

    if cancel_flag.load(Ordering::Relaxed) {
        handle_error("soundcloud.cancelled").await;
        return Ok(());
    }

    // ── STAGE 4: Embed ID3 Tags & Cover Art ──────────────────────────────────
    log_ev!("sc", trace_id, "tagging_enter", "path" => target_mp3.display().to_string());
    edit_status(t("soundcloud.tagging")).await;

    let cover_file_path = apply_soundcloud_id3_tags(&target_mp3, &meta, "cover", trace_id)
        .await
        .unwrap_or(None);

    if cancel_flag.load(Ordering::Relaxed) {
        handle_error("soundcloud.cancelled").await;
        return Ok(());
    }

    // ── STAGE 5: Upload Audio Message ────────────────────────────────────────
    log_ev!("sc", trace_id, "upload_enter", "file" => target_mp3.display().to_string());
    edit_status(t("soundcloud.uploading")).await;

    let is_fa = crate::i18n::current_lang() == "fa";
    let raw_date = meta.release_date.as_deref().unwrap_or("");
    let date_str = if raw_date.is_empty() {
        "-".to_string()
    } else {
        format_spotify_release_date(raw_date, is_fa)
    };

    let caption = tf(
        "soundcloud.done_caption",
        &[
            ("title", &md_escape(&meta.title)),
            ("artist", &md_escape(&meta.artist)),
            ("date", &md_escape(&date_str)),
        ],
    );

    let mut send_audio_params = SendAudioParams::builder()
        .chat_id(chat_id)
        .audio(FileUpload::InputFile(InputFile {
            path: target_mp3.clone(),
        }))
        .performer(meta.artist.clone())
        .title(meta.title.clone())
        .duration(meta.duration_secs as u32)
        .caption(apply_premium_to_md(&caption))
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(
            audio_separation_keyboard(),
        ))
        .build();

    if let Some(cover_path) = cover_file_path {
        send_audio_params.thumbnail = Some(FileUpload::InputFile(InputFile { path: cover_path }));
    }

    let upload_res = api.send_audio(&send_audio_params).await;

    match upload_res {
        Ok(_) => {
            log_ev!("sc", trace_id, "upload_ok", "=>" => "ok");

            let file_size = tokio::fs::metadata(&target_mp3)
                .await
                .map(|m| m.len() as i64)
                .unwrap_or(0);

            // Record stats & quota
            crate::stats::record_event_user(user_id, "soundcloud", "download", "ok", 1).await;
            crate::stats::record_event_global("soundcloud", "download", "ok", 1).await;

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
            log_ev!("sc", trace_id, "upload_fail", "err" => e.to_string());
            crate::stats::record_error_global("soundcloud", format!("upload_audio: {e}")).await;
            handle_error("soundcloud.download_failed").await;
        }
    }

    Ok(())
}

/// Download one SoundCloud track as MP3 into `job_dir/{stem}.mp3`.
///
/// Shared by the single-track handler and the playlist runner, so the CPU Broker
/// reservation, the cancel-aware wait loop and the exit logging live in exactly
/// one place. `stem` keeps playlist items from overwriting each other.
pub async fn download_soundcloud_audio(
    job_dir: &Path,
    stem: &str,
    sc_url: &str,
    user_id: i64,
    trace_id: u64,
    cancel_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<PathBuf> {
    let cores = acquire_cpu(user_id, trace_id).await;
    log_ev!("sc", trace_id, "cpu_acquired", "cores" => format!("{cores:?}"));

    let output_template = job_dir.join(format!("{stem}.%(ext)s"));
    let target_mp3 = job_dir.join(format!("{stem}.mp3"));

    let spawned = Command::new("yt-dlp")
        .arg("-x")
        .arg("--audio-format")
        .arg("mp3")
        .arg("--audio-quality")
        .arg("0")
        .arg("-o")
        .arg(&output_template)
        .arg(sc_url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn();

    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            release_cpu(cores, trace_id).await;
            log_ev!("sc", trace_id, "download_audio_fail", "err" => e.to_string());
            return Err(anyhow::anyhow!("Failed to spawn yt-dlp: {e}"));
        }
    };

    let dl_result = loop {
        if cancel_flag.load(Ordering::Relaxed) {
            let _ = child.start_kill();
            let _ = child.wait().await;
            release_cpu(cores, trace_id).await;
            log_ev!("sc", trace_id, "download_audio_cancelled", "=>" => "cancel");
            return Err(anyhow::anyhow!("cancelled"));
        }

        match tokio::time::timeout(Duration::from_millis(250), child.wait()).await {
            Ok(res) => break res,
            Err(_) => continue,
        }
    };

    log_ev!("sc", trace_id, "cpu_released", "cores" => format!("{cores:?}"));
    release_cpu(cores, trace_id).await;

    match dl_result {
        Ok(status) if status.success() && target_mp3.exists() => {
            log_ev!("sc", trace_id, "download_audio_ok", "path" => target_mp3.display().to_string());
            Ok(target_mp3)
        }
        Ok(status) => {
            log_ev!("sc", trace_id, "download_audio_fail", "status" => status.to_string());
            Err(anyhow::anyhow!("yt-dlp exit: {status}"))
        }
        Err(e) => {
            log_ev!("sc", trace_id, "download_audio_fail", "err" => e.to_string());
            Err(anyhow::anyhow!("yt-dlp wait: {e}"))
        }
    }
}

pub async fn apply_soundcloud_id3_tags(
    mp3_path: &Path,
    meta: &SoundcloudTrackMeta,
    cover_stem: &str,
    trace_id: u64,
) -> anyhow::Result<Option<PathBuf>> {
    let mut tag = Tag::new();
    tag.set_title(&meta.title);
    tag.set_artist(&meta.artist);
    tag.set_album("SoundCloud Single");

    let mut saved_cover_path: Option<PathBuf> = None;

    if let Some(cover_url) = &meta.thumbnail_url {
        match reqwest::get(cover_url).await {
            Ok(res) if res.status().is_success() => {
                if let Ok(bytes) = res.bytes().await {
                    let mime = if cover_url.contains(".png") {
                        "image/png".to_string()
                    } else {
                        "image/jpeg".to_string()
                    };
                    let picture = Picture {
                        mime_type: mime,
                        picture_type: PictureType::CoverFront,
                        description: "SoundCloud Cover".to_string(),
                        data: bytes.to_vec(),
                    };
                    tag.add_frame(picture);

                    if let Some(parent) = mp3_path.parent() {
                        let cover_file = parent.join(format!("{cover_stem}.jpg"));
                        if tokio::fs::write(&cover_file, &bytes).await.is_ok() {
                            saved_cover_path = Some(cover_file);
                        }
                    }

                    log_ev!(
                        "sc",
                        trace_id,
                        "id3_cover_embedded",
                        "bytes" => bytes.len()
                    );
                }
            }
            Ok(res) => {
                log_ev!(
                    "sc",
                    trace_id,
                    "id3_cover_http_err",
                    "status" => res.status().as_u16()
                );
            }
            Err(e) => {
                log_ev!("sc", trace_id, "id3_cover_fetch_err", "err" => e.to_string());
            }
        }
    }

    tag.write_to_path(mp3_path, id3::Version::Id3v24)?;
    log_ev!(
        "sc",
        trace_id,
        "id3_tags_saved",
        "path" => mp3_path.display().to_string()
    );
    Ok(saved_cover_path)
}
