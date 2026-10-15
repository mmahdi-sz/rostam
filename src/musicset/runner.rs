//! Album/playlist download queue execution.
//!
//! Each track uses its platform single-track pipeline, avoiding duplicated logic.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{DeleteMessageParams, SendAudioParams, SendDocumentParams},
};

use crate::common::cpu_broker::CpuBrokerGuard;
use crate::common::dir::TempDirGuard;
use crate::common::format::format_clock;
use crate::common::ticker::ProgressTicker;
use crate::database::postgresql::PostgresDatabase;
use crate::filecompress::config::{CompressAlgo, CompressConfig, CompressFmt};
use crate::filecompress::engine::run_compress;
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::musicset::{
    MS_SPLIT_MB, PendingSet, SetItems, edit_status, job_cancel_keyboard, register_cancel,
    send_status,
};

/// Readable title from SoundCloud URL fallback.
fn sc_slug(url: &str) -> String {
    let base = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    match base.rsplit('/').next() {
        Some(s) if !s.is_empty() => s.replace(['-', '_'], " "),
        _ => String::from("..."),
    }
}

#[cfg(test)]
mod tests {
    use super::sc_slug;

    #[test]
    fn slug_from_sc_url() {
        assert_eq!(
            sc_slug("https://soundcloud.com/zedbazi/az-ghadim?in=x/sets/y"),
            "az ghadim"
        );
        assert_eq!(sc_slug("https://soundcloud.com/a/b/"), "b");
    }
}

/// Downloaded and tagged track — ready for upload or archiving.
struct Track {
    mp3: PathBuf,
    cover: Option<PathBuf>,
    title: String,
    artist: String,
    duration_secs: u64,
}

pub async fn run_set_job(
    api: Bot,
    chat_id: i64,
    user_id: i64,
    trace_id: u64,
    pending: PendingSet,
    zip_mode: bool,
    // Edits mode selection message rather than creating a new one.
    status_msg_id: i32,
    database: Option<PostgresDatabase>,
) {
    let (cancel, _cancel_guard) = register_cancel(user_id);

    let root = if pending.domain == "sp" {
        crate::config::spotify_download_root()
    } else {
        crate::config::soundcloud_download_root()
    };
    let job_dir = PathBuf::from(root).join(format!("set_{}_{trace_id}", user_id.abs()));
    if let Err(e) = tokio::fs::create_dir_all(&job_dir).await {
        log_ev!("ms", trace_id, "workdir_fail", "err" => e.to_string());
        crate::stats::record_error_global("musicset", format!("workdir: {e}")).await;
        if status_msg_id > 0 {
            edit_status(&api, chat_id, status_msg_id, &t("musicset.io_error"), None).await;
        } else {
            let _ = send_status(&api, chat_id, &t("musicset.io_error")).await;
        }
        return;
    }
    let _dir_guard = TempDirGuard::from_path(job_dir.clone());

    let total = pending.len();
    log_ev!("ms", trace_id, "job_start", "tracks" => total, "mode" => if zip_mode { "zip" } else { "one" });

    let status_msg_id = if status_msg_id > 0 {
        status_msg_id
    } else {
        send_status(&api, chat_id, &t("musicset.starting")).await
    };
    edit_status(
        &api,
        chat_id,
        status_msg_id,
        &t("musicset.starting"),
        Some(job_cancel_keyboard()),
    )
    .await;

    // ── Live ticker: updates progress counter and clock every 3s ──
    let done_idx = Arc::new(AtomicUsize::new(0));
    let initial_name = match &pending.items {
        SetItems::Spotify(items) => {
            if let Some(first) = items.first() {
                if first.artist.is_empty() {
                    first.title.clone()
                } else {
                    format!("{} - {}", first.artist, first.title)
                }
            } else {
                String::from("...")
            }
        }
        SetItems::Soundcloud(urls) => {
            if let Some(first) = urls.first() {
                sc_slug(first)
            } else {
                String::from("...")
            }
        }
    };
    let cur_name = Arc::new(Mutex::new(initial_name));
    let ticker_done_idx = done_idx.clone();
    let ticker_cur_name = cur_name.clone();
    let title = md_escape(&pending.title);

    let ticker_handle = ProgressTicker::new(&api, chat_id, status_msg_id)
        .interval(Duration::from_secs(3))
        .with_cancel_flag(cancel.clone())
        .with_keyboard(job_cancel_keyboard())
        .spawn(move |elapsed| {
            let name = ticker_cur_name
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| String::from("..."));
            let text = tf(
                "musicset.progress",
                &[
                    ("title", &title),
                    (
                        "i",
                        &(ticker_done_idx.load(Ordering::Relaxed) + 1)
                            .min(total)
                            .to_string(),
                    ),
                    ("n", &total.to_string()),
                    ("name", &md_escape(&name)),
                    ("elapsed", &md_escape(&format_clock(elapsed.as_secs()))),
                ],
            );
            Some(apply_premium_to_md(&text))
        });

    // Spotify acquires CPU core for the entire queue; SoundCloud reserves per track.
    let mut cpu_guard = if pending.domain == "sp" {
        Some(CpuBrokerGuard::acquire(user_id, trace_id, "musicset").await)
    } else {
        None
    };
    let cores = Arc::new(
        cpu_guard
            .as_ref()
            .map(|g| g.cores().to_vec())
            .unwrap_or_default(),
    );

    const MAX_CONCURRENT_DOWNLOADS: usize = 3;
    const MAX_IN_FLIGHT: usize = 4;

    let dl_sem = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_DOWNLOADS));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(usize, tokio::task::JoinHandle<Option<Track>>)>(
        MAX_IN_FLIGHT,
    );

    let producer_cancel = cancel.clone();
    let producer_job_dir = job_dir.clone();
    let producer_cores = cores.clone();
    let producer_pending = pending.clone();

    tokio::spawn(async move {
        for idx in 0..total {
            if producer_cancel.load(Ordering::SeqCst) {
                break;
            }

            let task_dir = producer_job_dir.clone();
            let task_cores = producer_cores.clone();
            let task_cancel = producer_cancel.clone();
            let task_dl_sem = dl_sem.clone();

            let handle = match &producer_pending.items {
                SetItems::Spotify(items) => {
                    let item = items[idx].clone();
                    tokio::spawn(async move {
                        let _permit = match task_dl_sem.acquire().await {
                            Ok(p) => p,
                            Err(_) => return None,
                        };
                        if task_cancel.load(Ordering::SeqCst) {
                            return None;
                        }
                        let stem = format!("t{:03}", idx + 1);
                        fetch_spotify_track_file(
                            &task_dir,
                            &stem,
                            &item,
                            &task_cores,
                            trace_id,
                            &task_cancel,
                        )
                        .await
                    })
                }
                SetItems::Soundcloud(urls) => {
                    let url = urls[idx].clone();
                    tokio::spawn(async move {
                        let _permit = match task_dl_sem.acquire().await {
                            Ok(p) => p,
                            Err(_) => return None,
                        };
                        if task_cancel.load(Ordering::SeqCst) {
                            return None;
                        }
                        let stem = format!("t{:03}", idx + 1);
                        fetch_soundcloud_track_file(
                            &task_dir,
                            &stem,
                            &url,
                            user_id,
                            trace_id,
                            &task_cancel,
                        )
                        .await
                    })
                }
            };

            if tx.send((idx, handle)).await.is_err() {
                break;
            }
        }
    });

    let mut ready: Vec<Track> = Vec::new();
    let mut failed = 0usize;
    let mut cancelled = false;

    while let Some((idx, handle)) = rx.recv().await {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            handle.abort();
            break;
        }

        let track = match handle.await {
            Ok(Some(t)) => Some(t),
            Ok(None) => None,
            Err(_) => None,
        };

        let mut track = match track {
            Some(t) => t,
            None => {
                if cancel.load(Ordering::SeqCst) {
                    cancelled = true;
                    break;
                }
                failed += 1;
                log_ev!("ms", trace_id, "track_fail", "idx" => idx + 1);
                done_idx.store(idx + 1, Ordering::Relaxed);
                continue;
            }
        };

        // Rename downloaded file (e.g. t001.mp3) to real name (e.g. Artist - Title.mp3)
        let safe_name = crate::common::format::safe_music_filename(&track.artist, &track.title);
        let mut target_path = job_dir.join(&safe_name);
        let mut counter = 2;
        let base_stem = safe_name.strip_suffix(".mp3").unwrap_or(&safe_name);
        while target_path.exists() && target_path != track.mp3 {
            target_path = job_dir.join(format!("{base_stem} ({counter}).mp3"));
            counter += 1;
        }
        if target_path != track.mp3 {
            if tokio::fs::rename(&track.mp3, &target_path).await.is_ok() {
                track.mp3 = target_path;
            }
        }

        if let Ok(mut g) = cur_name.lock() {
            *g = if track.artist.is_empty() {
                track.title.clone()
            } else {
                format!("{} - {}", track.artist, track.title)
            };
        }

        if zip_mode {
            ready.push(track);
        } else {
            upload_track(&api, chat_id, user_id, trace_id, &track, &database).await;
            // Remove file after upload to free disk space.
            let _ = tokio::fs::remove_file(&track.mp3).await;
            if let Some(c) = &track.cover {
                let _ = tokio::fs::remove_file(c).await;
            }
        }
        done_idx.store(idx + 1, Ordering::Relaxed);
    }

    if !zip_mode {
        if let Some(mut g) = cpu_guard.take() {
            g.release().await;
        }
    }

    if cancelled {
        if let Some(mut g) = cpu_guard.take() {
            g.release().await;
        }
        ticker_handle.stop();
        log_ev!("ms", trace_id, "job_cancelled", "=>" => "cancel", "done" => done_idx.load(Ordering::Relaxed));
        delete_status(&api, chat_id, status_msg_id).await;
        // Uses MarkdownV2 with pre-escaped i18n text.
        send_status(&api, chat_id, &t("musicset.cancelled")).await;
        let _ = crate::bot::send_start_menu(&api, chat_id).await;
        return;
    }

    if zip_mode {
        if cpu_guard.is_none() {
            cpu_guard = Some(CpuBrokerGuard::acquire(user_id, trace_id, "musicset").await);
        }
        let cores = cpu_guard
            .as_ref()
            .map(|g| g.cores().to_vec())
            .unwrap_or_default();
        edit_status(
            &api,
            chat_id,
            status_msg_id,
            &t("musicset.zipping"),
            Some(job_cancel_keyboard()),
        )
        .await;
        let ok = archive_and_upload(
            &api,
            chat_id,
            user_id,
            trace_id,
            &job_dir,
            &pending.title,
            &ready,
            &cancel,
            &cores,
            &database,
        )
        .await;
        if let Some(mut g) = cpu_guard.take() {
            g.release().await;
        }
        if !ok {
            ticker_handle.stop();
            delete_status(&api, chat_id, status_msg_id).await;
            send_status(&api, chat_id, &t("musicset.zip_failed")).await;
            let _ = crate::bot::send_start_menu(&api, chat_id).await;
            return;
        }
    }

    ticker_handle.stop();

    let uploaded = total - failed;
    log_ev!("ms", trace_id, "job_done", "=>" => "ok", "uploaded" => uploaded, "failed" => failed);
    crate::stats::record_event_user(user_id, "musicset", "set", "ok", uploaded as i64).await;
    crate::stats::record_event_global("musicset", "set", "ok", uploaded as i64).await;

    delete_status(&api, chat_id, status_msg_id).await;
    let done_text = tf(
        "musicset.done",
        &[
            ("title", &md_escape(&pending.title)),
            ("n", &uploaded.to_string()),
            ("failed", &failed.to_string()),
        ],
    );
    send_status(&api, chat_id, &done_text).await;
    // Re-arm start menu prompt.
    let _ = crate::bot::send_start_menu(&api, chat_id).await;
}

async fn delete_status(api: &Bot, chat_id: i64, message_id: i32) {
    if message_id > 0 {
        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(message_id)
                    .build(),
            )
            .await;
    }
}

/// Spotify: metadata from embed/API, audio from best YouTube match.
async fn fetch_spotify_track_file(
    job_dir: &std::path::Path,
    stem: &str,
    item: &crate::spotify::client::SpotifySetItem,
    cores: &[i32],
    trace_id: u64,
    cancel: &Arc<AtomicBool>,
) -> Option<Track> {
    use crate::spotify::handle::{DlOutcome, run_yt_dlp_audio};

    log_ev!("ms", trace_id, "track_enter", "stem" => stem, "title" => &item.title, "artist" => &item.artist);
    let meta = crate::spotify::client::fetch_spotify_track(&item.track_id)
        .await
        .ok()?;

    let candidates = crate::spotify::search::find_best_youtube_candidates_fallback(
        &meta.primary_artist,
        &meta.artists_joined,
        &meta.album_name,
        &meta.title,
        meta.duration_ms,
        trace_id,
    )
    .await
    .ok()?;

    let mut dl_ok = false;
    for cand in candidates.iter().take(3) {
        if cancel.load(Ordering::SeqCst) {
            return None;
        }
        match run_yt_dlp_audio(job_dir, stem, &cand.webpage_url, cores, trace_id, cancel).await {
            DlOutcome::Ok => {
                dl_ok = true;
                break;
            }
            DlOutcome::Cancelled => return None,
            DlOutcome::Failed => {
                log_ev!(
                    "ms",
                    trace_id,
                    "cand_dl_fail_retry_next",
                    "url" => &cand.webpage_url,
                    "stem" => stem
                );
                continue;
            }
        }
    }

    if !dl_ok {
        return None;
    }

    let mp3 = job_dir.join(format!("{stem}.mp3"));
    if !mp3.exists() {
        return None;
    }
    let cover =
        crate::spotify::tagging::apply_id3_tags(&mp3, &meta, &format!("cover_{stem}"), trace_id)
            .await
            .unwrap_or(None);

    Some(Track {
        mp3,
        cover,
        title: meta.title,
        artist: meta.artists_joined,
        duration_secs: (meta.duration_ms + 500) / 1000,
    })
}

async fn fetch_soundcloud_track_file(
    job_dir: &std::path::Path,
    stem: &str,
    sc_url: &str,
    user_id: i64,
    trace_id: u64,
    cancel: &Arc<AtomicBool>,
) -> Option<Track> {
    let meta = crate::soundcloud::fetch::fetch_soundcloud_meta(trace_id, sc_url)
        .await
        .ok()?;

    let mp3 = crate::soundcloud::handle::download_soundcloud_audio(
        job_dir, stem, sc_url, user_id, trace_id, cancel,
    )
    .await
    .ok()?;
    let cover = crate::soundcloud::handle::apply_soundcloud_id3_tags(
        &mp3,
        &meta,
        &format!("cover_{stem}"),
        trace_id,
    )
    .await
    .unwrap_or(None);

    Some(Track {
        mp3,
        cover,
        title: meta.title,
        artist: meta.artist,
        duration_secs: meta.duration_secs,
    })
}

async fn add_traffic(database: &Option<PostgresDatabase>, user_id: i64, path: &std::path::Path) {
    let Some(db) = database else { return };
    let size = tokio::fs::metadata(path)
        .await
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    if let Ok(client) = db.get().await {
        if let Some(first_up) = crate::rank::quota::get_first_upload_at(&client, user_id).await {
            let _ = crate::rank::quota::add_traffic(&client, user_id, size, first_up).await;
        }
    }
}

async fn upload_track(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    trace_id: u64,
    track: &Track,
    database: &Option<PostgresDatabase>,
) {
    let caption = tf(
        "musicset.track_caption",
        &[
            ("title", &md_escape(&track.title)),
            ("artist", &md_escape(&track.artist)),
        ],
    );
    let mut params = SendAudioParams::builder()
        .chat_id(chat_id)
        .audio(FileUpload::InputFile(InputFile {
            path: track.mp3.clone(),
        }))
        .performer(track.artist.clone())
        .title(track.title.clone())
        .duration(track.duration_secs as u32)
        .caption(apply_premium_to_md(&caption))
        .parse_mode(ParseMode::MarkdownV2)
        .build();
    if let Some(cover) = &track.cover {
        params.thumbnail = Some(FileUpload::InputFile(InputFile {
            path: cover.clone(),
        }));
    }

    use crate::bot::send_file_with_upload_ticker;
    match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
        api,
        "sendAudio",
        &params,
        &track.mp3,
        chat_id,
        0,
        "transfer.stage.sending_audio",
        None,
    )
    .await
    {
        Ok(_) => {
            add_traffic(database, user_id, &track.mp3).await;
        }
        Err(e) => {
            log_ev!("ms", trace_id, "track_upload_fail", "err" => e.to_string());
            crate::stats::record_error_global("musicset", format!("upload_audio: {e}")).await;
        }
    }
}

/// 7z level 9 with splitting; uploads each part as document.
#[allow(clippy::too_many_arguments)]
async fn archive_and_upload(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    trace_id: u64,
    job_dir: &std::path::Path,
    set_title: &str,
    tracks: &[Track],
    cancel: &Arc<AtomicBool>,
    cores: &[i32],
    database: &Option<PostgresDatabase>,
) -> bool {
    if tracks.is_empty() {
        return false;
    }
    let inputs: Vec<PathBuf> = tracks.iter().map(|t| t.mp3.clone()).collect();
    let config = CompressConfig {
        fmt: CompressFmt::SevenZ,
        algo: CompressAlgo::Lzma2,
        level: 9,
        password: None,
        split_mb: Some(MS_SPLIT_MB),
        obfuscate: false,
        solid: false,
    };

    log_ev!("ms", trace_id, "archive_enter", "files" => inputs.len());
    // Music sets render their own per-track status line, so the archiver's
    // percent goes nowhere — a throwaway sink keeps one engine signature.
    let archive_progress =
        std::sync::Arc::new(crate::filecompress::progress::JobProgress::default());
    let result = match run_compress(
        job_dir,
        &config,
        &inputs,
        Duration::from_secs(3600),
        cores,
        trace_id,
        cancel,
        &archive_progress,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            log_ev!("ms", trace_id, "archive_enter", "=>" => "fail", "err" => format!("{e}"));
            crate::stats::record_error_global("musicset", format!("compress: {e}")).await;
            return false;
        }
    };
    log_ev!("ms", trace_id, "archive_enter", "=>" => "ok", "parts" => result.output_paths.len());

    let safe_base = crate::common::format::safe_archive_stem(set_title);
    let parts = result.output_paths.len();
    let mut final_paths = Vec::with_capacity(parts);
    for (i, p) in result.output_paths.iter().enumerate() {
        let new_name = if parts == 1 {
            format!("{safe_base}.7z")
        } else {
            format!("{safe_base}.7z.{:03}", i + 1)
        };
        let target = p.with_file_name(&new_name);
        if target != *p {
            if tokio::fs::rename(p, &target).await.is_ok() {
                final_paths.push(target);
                continue;
            }
        }
        final_paths.push(p.clone());
    }

    for (i, path) in final_paths.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        let caption = if parts > 1 {
            tf(
                "musicset.part_caption",
                &[("i", &(i + 1).to_string()), ("n", &parts.to_string())],
            )
        } else {
            format!("📦 *{}*", md_escape(set_title))
        };
        let params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(path.clone())
            .caption(apply_premium_to_md(&caption))
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        use crate::bot::send_file_with_upload_ticker;
        if let Err(e) = send_file_with_upload_ticker::<_, frankenstein::types::Message>(
            api,
            "sendDocument",
            &params,
            path,
            chat_id,
            0,
            "transfer.stage.sending_document",
            None,
        )
        .await
        {
            log_ev!("ms", trace_id, "part_upload_fail", "err" => e.to_string());
            crate::stats::record_error_global("musicset", format!("upload_part: {e}")).await;
            return false;
        }
        add_traffic(database, user_id, path).await;
    }
    true
}
