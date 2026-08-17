use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{
        DeleteMessageParams, PinChatMessageParams, SendMessageParams, UnpinChatMessageParams,
    },
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Notify;

use crate::i18n::{entities_for_text, t, tf};
use crate::stats;

use super::super::trace::log_trace;
use super::cancel::{UnregisterGuard, register_cancel, unregister_cancel};
use super::helpers::{
    cleanup_dir, fetch_thumbnail, maybe_send_non_h264_notice, pick_largest_file, quality_label_for,
    sanitize_audio_filename, sanitize_video_filename, send_subtitle_files,
};
use super::progress::{ProgressSnapshot, format_progress_body, parse_progress_line};
use super::selection_helpers::find_format;
use super::split::split_video;
use super::status::{edit_progress_status, edit_status};
use super::store::take_request;
use super::types::{Selection, SubtitleMode};
use super::upload::{
    MediaPayload, build_part_doc_params, build_part_params, build_single_doc_params,
    build_single_params, send_audio_file, send_media_with_progress,
};
use crate::youtube::types::VideoCodec;

pub const EDIT_THROTTLE: Duration = Duration::from_secs(1);
const MAX_SIZE_MB: u64 = 2000;
const TARGET_PART_MB: u64 = 1700;

pub fn spawn_download(
    api: Bot,
    request_id: u64,
    selection: Selection,
    status_chat_id: i64,
    status_message_id: i32,
) {
    let cancel = register_cancel(request_id);
    crate::app::spawn_user_task(async move {
        // Check whether playlist or single video.
        if let Some(req_peek) = super::store::get_request(request_id) {
            if req_peek.is_playlist && !req_peek.playlist_items.is_empty() {
                run_playlist_download(
                    api,
                    request_id,
                    selection,
                    status_chat_id,
                    status_message_id,
                    cancel,
                )
                .await;
                return;
            }
        }
        run_download(
            api,
            request_id,
            selection,
            status_chat_id,
            status_message_id,
            cancel,
        )
        .await
    });
}

async fn run_playlist_download(
    api: Bot,
    request_id: u64,
    selection: Selection,
    status_chat_id: i64,
    status_message_id: i32,
    _cancel: Arc<Notify>,
) {
    let Some(mut req) = super::store::take_request(request_id) else {
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            t("youtube.download.request_expired"),
        )
        .await;
        unregister_cancel(request_id);
        return;
    };
    let _cancel_guard = UnregisterGuard(request_id);
    let trace_id = req.trace_id;
    let user_id = req.user_id.unwrap_or(0);
    let stats_job_id = stats::record_download_start(user_id, "youtube").await;
    let _active_dl_guard = crate::metrics::ActiveDownloadGuard::new();
    let _duration_guard = crate::metrics::RequestDurationGuard::new("youtube");

    let user_rank = if user_id > 0 {
        if let Some(db) = stats::get_db_client().await {
            crate::rank::effective_rank(&db, user_id).await
        } else {
            crate::rank::types::Rank::Dalavar
        }
    } else {
        crate::rank::types::Rank::Dalavar
    };

    if let Some(limit) = user_rank.playlist_limit() {
        let limit = limit as usize;
        if req.playlist_items.len() > limit {
            let original_count = req.playlist_items.len();
            req.playlist_items.truncate(limit);
            log_trace(
                trace_id,
                "playlist_items_truncated",
                &format!(
                    "user_id={user_id} rank={} original={original_count} limit={limit}",
                    user_rank.as_str()
                ),
            );
            if limit == 0 {
                crate::rank::paywall::block_feature(
                    &api,
                    status_chat_id,
                    "دانلود پلی‌لیست",
                    crate::rank::types::Rank::Sepahbod,
                )
                .await;
                return;
            } else {
                let note = format!(
                    "⚠️ به دلیل محدودیت سطح کاربری ({})، فقط {limit} ویدیوی اول از {original_count} ویدیو دانلود می‌شود.",
                    user_rank.as_str()
                );
                let _ = api
                    .send_message(
                        &SendMessageParams::builder()
                            .chat_id(status_chat_id)
                            .text(note)
                            .build(),
                    )
                    .await;
            }
        }
    }

    let total_videos = req.playlist_items.len();

    log_trace(
        trace_id,
        "playlist_download_begin",
        &format!(
            "total_videos={total_videos} height={} codec={}",
            selection.height,
            selection.codec.key()
        ),
    );

    // Start message
    let start_msg = tf(
        "youtube.download.playlist.start",
        &[("count", &total_videos.to_string())],
    );
    edit_status(&api, status_chat_id, status_message_id, start_msg).await;

    // Pin status message so it is not lost among sent files.
    let _ = api
        .pin_chat_message(
            &PinChatMessageParams::builder()
                .chat_id(status_chat_id)
                .message_id(status_message_id)
                .disable_notification(true)
                .build(),
        )
        .await;

    let mut sent = 0usize;
    // (video_num, title, reason) for failed video deliveries
    let mut failures: Vec<(usize, String, String)> = Vec::new();

    for (idx, item) in req.playlist_items.iter().enumerate() {
        let video_num = idx + 1;
        let title_short = if item.title.chars().count() > 50 {
            item.title.chars().take(50).collect::<String>() + "…"
        } else {
            item.title.clone()
        };
        let duration_str = item
            .duration
            .map(super::super::format::format_duration)
            .unwrap_or_else(|| "-".to_string());
        let progress_msg = tf(
            "youtube.download.playlist.progress",
            &[
                ("num", &video_num.to_string()),
                ("total", &total_videos.to_string()),
                ("title", &title_short),
                ("duration", &duration_str),
            ],
        );
        edit_status(&api, status_chat_id, status_message_id, progress_msg).await;

        // Build video URL
        let video_url = format!("https://www.youtube.com/watch?v={}", item.id);

        // Download and send single video to chat
        match download_single_playlist_item(
            &api,
            &video_url,
            &req,
            &selection,
            &item.title,
            video_num,
            total_videos,
            trace_id,
        )
        .await
        {
            Ok(bytes) => {
                sent += 1;
                if let Some(job_id) = stats_job_id {
                    stats::record_upload_done(
                        job_id,
                        user_id,
                        bytes as i64,
                        None,
                        Some(sent as i32),
                    )
                    .await;
                }
            }
            Err(reason) => {
                log_trace(
                    trace_id,
                    "playlist_item_not_sent",
                    &format!("num={video_num} id={} reason={reason}", item.id),
                );
                failures.push((video_num, title_short.clone(), reason));
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    // Final report: list failed videos with reason, followed by part sent/total.
    const MAX_LISTED: usize = 15;
    let mut lines: Vec<String> = Vec::new();
    if failures.is_empty() {
        lines.push(t("youtube.download.playlist.done_header"));
    } else {
        lines.push(t("youtube.download.playlist.failures_header"));
        for (num, title, reason) in failures.iter().take(MAX_LISTED) {
            lines.push(tf(
                "youtube.download.playlist.item_failed",
                &[
                    ("num", &num.to_string()),
                    ("title", title),
                    ("reason", reason),
                ],
            ));
        }
        if failures.len() > MAX_LISTED {
            lines.push(tf(
                "youtube.download.playlist.more_failures",
                &[("n", &(failures.len() - MAX_LISTED).to_string())],
            ));
        }
    }
    lines.push(String::new());
    let width = total_videos.to_string().len();
    lines.push(tf(
        "youtube.download.playlist.part_line",
        &[
            ("sent", &format!("{sent:0width$}")),
            ("total", &format!("{total_videos:0width$}")),
        ],
    ));
    edit_status(&api, status_chat_id, status_message_id, lines.join("\n")).await;

    let is_h264 = selection.codec == crate::youtube::types::VideoCodec::H264;
    if sent > 0 && !is_h264 {
        let codec_name = t(selection.codec.label_key());
        maybe_send_non_h264_notice(&api, req.chat_id, &codec_name, trace_id).await;
    }

    // Unpin status message once completed.
    let _ = api
        .unpin_chat_message(
            &UnpinChatMessageParams::builder()
                .chat_id(status_chat_id)
                .message_id(status_message_id)
                .build(),
        )
        .await;

    log_trace(
        trace_id,
        "playlist_download_end",
        &format!("sent={sent} failed={} total={total_videos}", failures.len()),
    );
    if let Some(job_id) = stats_job_id {
        stats::record_download_done(job_id, (sent as i64) * 100_000_000, None, None, None).await;
    }
}

/// Downloads a playlist video in a separate directory, sends it to user, and cleans up.
/// `Ok(bytes)` = successful upload; `Err(reason)` = failure reason for user report.
async fn download_single_playlist_item(
    api: &Bot,
    video_url: &str,
    req: &super::types::YoutubeRequest,
    selection: &Selection,
    item_title: &str,
    video_num: usize,
    total_videos: usize,
    trace_id: u64,
) -> Result<u64, String> {
    let height = selection.height;
    let codec = selection.codec;

    // Isolated directory for each video to prevent collisions
    let dir = PathBuf::from(format!(
        "{}/{trace_id}/pl_{video_num}",
        crate::config::youtube_download_root()
    ));
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        log_trace(
            trace_id,
            "playlist_item_mkdir_failed",
            &format!("url={video_url} err={e}"),
        );
        return Err(e.to_string());
    }

    // Use height/codec format selector instead of fixed format_id to select format per video.
    let vcodec_prefix = match codec {
        super::super::types::VideoCodec::H264 => "avc1",
        super::super::types::VideoCodec::H265 => "hevc",
        super::super::types::VideoCodec::Vp9 => "vp9",
        super::super::types::VideoCodec::Av1 => "av01",
    };
    let is_h264 = codec == super::super::types::VideoCodec::H264;
    let merge_format = if is_h264 { "mp4" } else { "mkv" };
    let format_spec = format!(
        "bestvideo[height<={height}][vcodec^={vcodec_prefix}]+bestaudio/bestvideo[height<={height}]+bestaudio/best[height<={height}]/best"
    );

    let output_template = format!("{}/%(id)s.%(ext)s", dir.display());
    let mut cmd = tokio::process::Command::new("yt-dlp");
    cmd.arg("--js-runtimes")
        .arg(format!("deno:{}", crate::config::deno_path()))
        .arg("--cookies-from-browser")
        .arg(&req.cookie_spec)
        .arg("--extractor-args")
        .arg("youtubetab:skip=authcheck")
        .arg("--no-warnings")
        .arg("--no-playlist")
        .arg("-f")
        .arg(&format_spec)
        .arg("--merge-output-format")
        .arg(merge_format)
        .arg("--print")
        .arg("after_move:filepath")
        .arg("-o")
        .arg(&output_template);

    // Handle subtitles if present
    if !selection.subtitle_langs.is_empty() {
        let sub_langs = selection.subtitle_langs.join(",");
        cmd.arg("--write-subs")
            .arg("--write-auto-subs")
            .arg("--sub-langs")
            .arg(&sub_langs);
        if let SubtitleMode::Embedded = selection.subtitle_mode {
            cmd.arg("--embed-subs");
        } else {
            cmd.arg("--convert-subs").arg("srt");
        }
    }

    cmd.arg(video_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = match cmd.output().await {
        Ok(o) => o,
        Err(e) => {
            log_trace(
                trace_id,
                "playlist_item_spawn_failed",
                &format!("url={video_url} error={e}"),
            );
            cleanup_dir(&dir, trace_id).await;
            return Err(e.to_string());
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lower = stderr.to_ascii_lowercase();
        let reason = if lower.contains("members-only") || lower.contains("join this channel") {
            t("youtube.download.playlist.reason_members_only")
        } else {
            stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| {
                    let l = l.trim();
                    if l.chars().count() > 120 {
                        l.chars().take(120).collect::<String>() + "…"
                    } else {
                        l.to_string()
                    }
                })
                .unwrap_or_else(|| t("youtube.download.playlist.reason_no_format"))
        };
        log_trace(
            trace_id,
            "playlist_item_download_failed",
            &format!("url={video_url} reason={reason}"),
        );
        cleanup_dir(&dir, trace_id).await;
        return Err(reason);
    }

    // Output file: from after_move:filepath line or largest file in directory
    let stdout = String::from_utf8_lossy(&output.stdout);
    let path = stdout
        .lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with('/') && !l.ends_with(".srt") && !l.ends_with(".vtt"))
        .last()
        .map(|s| s.to_string())
        .filter(|p| std::path::Path::new(p).exists())
        .or_else(|| pick_largest_file(&dir));
    let Some(path) = path else {
        log_trace(
            trace_id,
            "playlist_item_no_file",
            &format!("url={video_url}"),
        );
        cleanup_dir(&dir, trace_id).await;
        return Err(t("youtube.download.playlist.reason_no_file"));
    };

    if selection.subtitle_mode == SubtitleMode::Embedded && !selection.subtitle_langs.is_empty() {
        super::helpers::fix_embedded_subtitle_flags(&path, trace_id).await;
    }

    let file_size_bytes = tokio::fs::metadata(&path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    // Caption formatted like single download
    let quality_label = quality_label_for(height);
    let codec_name = t(codec.label_key());
    let bitrate_str = find_format(req, height, codec)
        .and_then(|f| f.bitrate)
        .map(|b| format!("{b:.0}"))
        .unwrap_or_else(|| "?".to_string());
    let sub_tag = match (selection.subtitle_mode, selection.subtitle_langs.is_empty()) {
        (SubtitleMode::Hardsub, false) => " | Hardsub",
        (SubtitleMode::Embedded, false) => " | Softsub",
        _ => "",
    };
    let thumb_path = fetch_thumbnail(&req.thumbnail_url, &dir, trace_id).await;

    let clean_name = sanitize_video_filename(
        item_title,
        &quality_label,
        &codec_name,
        &bitrate_str,
        merge_format,
    );
    let new_path = dir.join(&clean_name);
    let path = if tokio::fs::rename(&path, &new_path).await.is_ok() {
        new_path.to_string_lossy().into_owned()
    } else {
        path
    };
    let bot_username = crate::config::bot_username().to_string();
    let width = total_videos.to_string().len();
    let caption = tf(
        "youtube.download.playlist.caption",
        &[
            ("title", item_title),
            ("part", &format!("{video_num:0width$}")),
            ("total", &format!("{total_videos:0width$}")),
            ("quality", &quality_label),
            ("codec", &codec_name),
            ("bitrate", &bitrate_str),
            ("sub_tag", sub_tag),
            ("username", &bot_username),
            ("url", video_url),
        ],
    );
    let caption_entities = entities_for_text(&caption);
    let send_res = if is_h264 {
        let params = build_single_params(
            &path,
            req.chat_id,
            &thumb_path,
            caption,
            caption_entities,
            height,
            None,
        );
        let progress = crate::bot::transfer::TransferProgress::new(0);
        crate::bot::transfer::send_params_metered::<
            _,
            frankenstein::response::MethodResponse<frankenstein::types::Message>,
        >(&api.api_url, "sendVideo", &params, &progress, None)
        .await
        .map(|_| ())
    } else {
        let params =
            build_single_doc_params(&path, req.chat_id, &thumb_path, caption, caption_entities);
        let progress = crate::bot::transfer::TransferProgress::new(0);
        crate::bot::transfer::send_params_metered::<
            _,
            frankenstein::response::MethodResponse<frankenstein::types::Message>,
        >(&api.api_url, "sendDocument", &params, &progress, None)
        .await
        .map(|_| ())
    };

    cleanup_dir(&dir, trace_id).await;
    match send_res {
        Ok(_) => {
            log_trace(
                trace_id,
                "playlist_item_upload_ok",
                &format!("num={video_num}"),
            );
            Ok(file_size_bytes)
        }
        Err(e) => {
            log_trace(
                trace_id,
                "playlist_item_upload_failed",
                &format!("num={video_num} err={e}"),
            );
            Err(e.to_string())
        }
    }
}

async fn run_download(
    api: Bot,
    request_id: u64,
    selection: Selection,
    status_chat_id: i64,
    status_message_id: i32,
    cancel: Arc<Notify>,
) {
    let height = selection.height;
    let codec = selection.codec;
    let Some(req) = take_request(request_id) else {
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            t("youtube.download.request_expired"),
        )
        .await;
        unregister_cancel(request_id);
        return;
    };
    let _cancel_guard = UnregisterGuard(request_id);
    let mut cancel_fut = std::pin::pin!(cancel.notified());
    let trace_id = req.trace_id;
    let user_id = req.user_id.unwrap_or(0);

    // Record download start
    let stats_job_id = stats::record_download_start(user_id, "youtube").await;
    let _active_dl_guard = crate::metrics::ActiveDownloadGuard::new();
    let _duration_guard = crate::metrics::RequestDurationGuard::new("youtube");

    let user_rank = if user_id > 0 {
        if let Some(db) = stats::get_db_client().await {
            crate::rank::effective_rank(&db, user_id).await
        } else {
            crate::rank::types::Rank::Dalavar
        }
    } else {
        crate::rank::types::Rank::Dalavar
    };

    if let Some(max) = user_rank.max_yt_quality()
        && height > max
    {
        log_trace(
            trace_id,
            "quality_paywall_download_aborted",
            &format!(
                "user_id={user_id} height={height} max={max} rank={}",
                user_rank.as_str()
            ),
        );
        let limit = format!("{max}p");
        let min_rank = crate::rank::types::Rank::min_for_quality(height);
        crate::rank::paywall::block_limit(&api, status_chat_id, &limit, min_rank).await;
        unregister_cancel(request_id);
        return;
    }

    let is_audio = selection.audio_only.is_some();
    let quality_label = if let Some(aq) = selection.audio_only {
        t(aq.label_key())
    } else {
        quality_label_for(height)
    };

    log_trace(
        trace_id,
        "download_begin",
        &format!(
            "request_id={request_id} is_audio={is_audio} height={height} codec={} url={}",
            codec.key(),
            req.webpage_url
        ),
    );

    let (format_spec, merge_format, is_h264) = if let Some(aq) = selection.audio_only {
        (aq.format_spec().to_string(), "mp3", false)
    } else {
        let Some(fmt) = find_format(&req, height, codec) else {
            log_trace(
                trace_id,
                "download_format_missing",
                &format!("height={height} codec={}", codec.key()),
            );
            edit_status(
                &api,
                status_chat_id,
                status_message_id,
                t("youtube.download.failed"),
            )
            .await;
            return;
        };
        let format_id = fmt.format_id.clone();
        let is_h264 = codec == super::super::types::VideoCodec::H264;
        let merge_format = if is_h264 { "mp4" } else { "mkv" };
        let format_spec = match codec {
            super::super::types::VideoCodec::H264 => format!("{format_id}+bestaudio/best"),
            _ => {
                format!(
                    "{format_id}+bestaudio/{format_id}/bestvideo[height<={height}]+bestaudio/best"
                )
            }
        };
        (format_spec, merge_format, is_h264)
    };

    let dir = PathBuf::from(format!(
        "{}/{trace_id}",
        crate::config::youtube_download_root()
    ));
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        log_trace(trace_id, "download_mkdir_failed", &e.to_string());
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            t("youtube.download.failed"),
        )
        .await;
        return;
    }

    let output_template = format!("{}/%(id)s.%(ext)s", dir.display());

    let progress_template = format!(
        "YT_PROGRESS|%(progress._percent_str)s|%(progress._downloaded_bytes_str)s|%(progress._total_bytes_str)s|%(progress._total_bytes_estimate_str)s|%(progress._speed_str)s|%(progress._eta_str)s|%(progress._elapsed_str)s"
    );

    let initial = ProgressSnapshot {
        percent: "0.0%".into(),
        downloaded: "0B".into(),
        total: "?".into(),
        speed: "?".into(),
        eta: "?".into(),
        elapsed: "00:00".into(),
        percent_int: 0,
    };
    edit_progress_status(
        &api,
        status_chat_id,
        status_message_id,
        format_progress_body(&initial, &quality_label),
        request_id,
    )
    .await;

    let postprocess_template = progress_template.clone();
    let mut cmd = tokio::process::Command::new("yt-dlp");
    cmd.arg("--js-runtimes")
        .arg(format!("deno:{}", crate::config::deno_path()))
        .arg("--cookies-from-browser")
        .arg(&req.cookie_spec)
        .arg("--extractor-args")
        .arg("youtubetab:skip=authcheck")
        .arg("--no-warnings")
        .arg("--no-playlist")
        .arg("--progress")
        .arg("--no-color")
        .arg("-f")
        .arg(&format_spec);

    if is_audio {
        cmd.arg("--extract-audio")
            .arg("--audio-format")
            .arg("mp3")
            .arg("--audio-quality")
            .arg("0");
    } else {
        cmd.arg("--merge-output-format").arg(merge_format);
    }

    cmd.arg("--newline")
        .arg("--progress-template")
        .arg(format!("download:{progress_template}"))
        .arg("--progress-template")
        .arg(format!("postprocess:{postprocess_template}"))
        .arg("--print")
        .arg("after_move:filepath")
        .arg("-o")
        .arg(&output_template);

    if !is_audio && !selection.subtitle_langs.is_empty() {
        let sub_langs = selection.subtitle_langs.join(",");
        // Most YouTube subtitle languages (e.g. fa) exist ONLY as auto-generated
        // captions, so both --write-subs and --write-auto-subs are required —
        // otherwise yt-dlp reports "no subtitles for the requested languages"
        // and produces no subtitle output at all.
        // Always convert to .srt and never use yt-dlp's own --embed-subs: the
        // post-download `embed_subtitles()` pass is the single place that
        // muxes subtitles into the mp4 (it also has to add translated tracks
        // yt-dlp doesn't know about). Letting yt-dlp embed here too would
        // double-embed the same language when a translation pass follows.
        cmd.arg("--write-subs")
            .arg("--write-auto-subs")
            .arg("--sub-langs")
            .arg(&sub_langs)
            .arg("--convert-subs")
            .arg("srt")
            .arg("--ignore-errors");
        log_trace(
            trace_id,
            "download_subtitle_args",
            &format!(
                "sub_langs={sub_langs} mode={:?} write_auto=true ignore_errors=true",
                selection.subtitle_mode
            ),
        );
    }

    cmd.arg(&req.webpage_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    log_trace(
        trace_id,
        "download_args",
        &format!("cookie_spec={} format_spec={format_spec}", req.cookie_spec),
    );

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log_trace(trace_id, "download_spawn_failed", &e.to_string());
            edit_status(
                &api,
                status_chat_id,
                status_message_id,
                t("youtube.download.failed"),
            )
            .await;
            return;
        }
    };

    let Some(stdout) = child.stdout.take() else {
        log_trace(trace_id, "download_spawn_failed", "piped stdout missing");
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            t("youtube.download.failed"),
        )
        .await;
        return;
    };
    let Some(stderr) = child.stderr.take() else {
        log_trace(trace_id, "download_spawn_failed", "piped stderr missing");
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            t("youtube.download.failed"),
        )
        .await;
        return;
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(&'static str, String)>(64);
    let tx_out = tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx_out.send(("stdout", line)).await;
        }
    });
    let tx_err = tx;
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx_err.send(("stderr", line)).await;
        }
    });

    let mut filepath: Option<String> = None;
    let mut last_edit = std::time::Instant::now() - EDIT_THROTTLE;
    let mut last_percent_int = -1;
    let mut stderr_tail = String::new();

    loop {
        tokio::select! {
            msg = rx.recv() => {
                let Some((source, line)) = msg else { break; };
                if let Some(snap) = parse_progress_line(&line) {
                    let now = std::time::Instant::now();
                    if snap.percent_int != last_percent_int && now.duration_since(last_edit) >= EDIT_THROTTLE {
                        last_percent_int = snap.percent_int;
                        last_edit = now;
                        log_trace(trace_id, "download_progress", &format!(
                            "src={source} percent={} downloaded={} total={} speed={} eta={}",
                            snap.percent, snap.downloaded, snap.total, snap.speed, snap.eta
                        ));
                        edit_progress_status(&api, status_chat_id, status_message_id,
                            format_progress_body(&snap, &quality_label), request_id).await;
                    }
                    continue;
                }
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() { continue; }
                let is_subtitle = trimmed.ends_with(".srt") || trimmed.ends_with(".vtt");
                if source == "stdout" && trimmed.starts_with('/') && !is_subtitle && tokio::fs::metadata(&trimmed).await.is_ok() {
                    filepath = Some(trimmed.clone());
                    log_trace(trace_id, "download_filepath", &trimmed);
                } else if source == "stderr" {
                    stderr_tail = trimmed.clone();
                    log_trace(trace_id, "yt_dlp_stderr", &trimmed);
                } else {
                    log_trace(trace_id, "yt_dlp_stdout", &trimmed);
                }
            }
            _ = &mut cancel_fut => {
                log_trace(trace_id, "download_cancelled", "cancel signal during download");
                let _ = child.kill().await;
                edit_status(&api, status_chat_id, status_message_id, t("youtube.download.cancelled")).await;
                cleanup_dir(&dir, trace_id).await;
                return;
            }
        }
    }
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    let status = match child.wait().await {
        Ok(s) => s,
        Err(e) => {
            log_trace(trace_id, "download_wait_failed", &e.to_string());
            edit_status(
                &api,
                status_chat_id,
                status_message_id,
                t("youtube.download.failed"),
            )
            .await;
            return;
        }
    };

    if !status.success() {
        let err = if stderr_tail.is_empty() {
            format!("exit {status}")
        } else {
            stderr_tail
        };
        log_trace(
            trace_id,
            "download_failed",
            &format!("status={status} err={err}"),
        );
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            t("youtube.download.failed"),
        )
        .await;
        cleanup_dir(&dir, trace_id).await;
        return;
    }

    let mut path = match filepath.or_else(|| pick_largest_file(&dir)) {
        Some(p) => p,
        None => {
            log_trace(trace_id, "download_no_filepath", "no output file located");
            edit_status(
                &api,
                status_chat_id,
                status_message_id,
                t("youtube.download.failed"),
            )
            .await;
            cleanup_dir(&dir, trace_id).await;
            return;
        }
    };

    log_trace(trace_id, "download_complete", &format!("path={path}"));

    if !is_audio {
        let sub_pipeline_res = super::helpers::process_subtitle_pipeline(
            &api,
            status_chat_id,
            status_message_id,
            &dir,
            &path,
            &selection,
            &req.cookie_spec,
            &req.webpage_url,
            req.duration,
            trace_id,
            user_id,
        )
        .await;

        if let super::helpers::SubtitlePipelineResult::VideoUpdated(new_path) = sub_pipeline_res {
            path = new_path;
        }
    }

    let file_size_bytes = tokio::fs::metadata(&path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if let Some(jid) = stats_job_id {
        let duration_i32 = req.duration.map(|d| d as i32);
        let bitrate = req.duration.and_then(|d| {
            if d > 0 {
                Some((file_size_bytes as i64 * 8) / (d as i64))
            } else {
                None
            }
        });
        stats::record_download_done(jid, file_size_bytes as i64, duration_i32, bitrate, None).await;
    }

    let hardsub_transcoded = matches!(selection.subtitle_mode, SubtitleMode::Hardsub)
        && !selection.subtitle_langs.is_empty();
    let codec_name = if is_audio {
        "MP3".to_string()
    } else if hardsub_transcoded {
        t(VideoCodec::H264.label_key())
    } else {
        t(selection.codec.label_key())
    };
    let bitrate_str = if is_audio {
        req.duration
            .and_then(|d| {
                if d > 0 {
                    Some(format!(
                        "{:.0}",
                        (file_size_bytes as f64 * 8.0) / (d as f64 * 1000.0)
                    ))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "?".to_string())
    } else if hardsub_transcoded {
        "?".to_string()
    } else {
        find_format(&req, height, codec)
            .and_then(|f| f.bitrate)
            .map(|b| format!("{b:.0}"))
            .unwrap_or_else(|| "?".to_string())
    };
    let sub_tag = match (selection.subtitle_mode, selection.subtitle_langs.is_empty()) {
        (SubtitleMode::Hardsub, false) => " | Hardsub",
        (SubtitleMode::Embedded, false) => " | Softsub",
        _ => "",
    };
    let thumb_path = fetch_thumbnail(&req.thumbnail_url, &dir, trace_id).await;

    let clean_name = if is_audio {
        sanitize_audio_filename(&req.title, &quality_label, "mp3")
    } else {
        sanitize_video_filename(
            &req.title,
            &quality_label,
            &codec_name,
            &bitrate_str,
            merge_format,
        )
    };
    let new_path = dir.join(&clean_name);
    if tokio::fs::rename(&path, &new_path).await.is_ok() {
        log_trace(
            trace_id,
            "download_renamed",
            &format!("old={path} new={}", new_path.display()),
        );
        path = new_path.to_string_lossy().into_owned();
    }

    let file_size_mb = file_size_bytes / (1024 * 1024);
    log_trace(
        trace_id,
        "upload_size_check",
        &format!("size_mb={file_size_mb} max_mb={MAX_SIZE_MB}"),
    );

    let upload_ok = if file_size_mb > MAX_SIZE_MB {
        let num_parts = ((file_size_mb + TARGET_PART_MB - 1) / TARGET_PART_MB) as usize;
        log_trace(
            trace_id,
            "split_needed",
            &format!("size_mb={file_size_mb} parts={num_parts}"),
        );
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            tf(
                "youtube.download.splitting",
                &[("parts", &num_parts.to_string())],
            ),
        )
        .await;

        let part_paths = match split_video(&path, &dir, num_parts, req.duration, trace_id).await {
            Ok(p) => p,
            Err(e) => {
                log_trace(trace_id, "split_failed", &e);
                edit_status(
                    &api,
                    status_chat_id,
                    status_message_id,
                    t("youtube.download.split_failed"),
                )
                .await;
                cleanup_dir(&dir, trace_id).await;
                return;
            }
        };

        let total = part_paths.len();
        let mut all_ok = true;
        for (i, part_path) in part_paths.iter().enumerate() {
            let part_num = i + 1;
            let part_size_mb = tokio::fs::metadata(part_path)
                .await
                .map(|m| m.len() / (1024 * 1024))
                .unwrap_or(0);
            log_trace(
                trace_id,
                "split_part_size",
                &format!("part={part_num}/{total} size_mb={part_size_mb}"),
            );
            if part_size_mb > MAX_SIZE_MB {
                log_trace(
                    trace_id,
                    "split_part_too_large",
                    &format!("part={part_num} size_mb={part_size_mb}"),
                );
                edit_status(
                    &api,
                    status_chat_id,
                    status_message_id,
                    tf(
                        "youtube.download.split_failed",
                        &[("error", &format!("part {part_num} still {part_size_mb}MB"))],
                    ),
                )
                .await;
                cleanup_dir(&dir, trace_id).await;
                return;
            }

            edit_status(
                &api,
                status_chat_id,
                status_message_id,
                tf(
                    "youtube.download.uploading_part",
                    &[
                        ("part", &part_num.to_string()),
                        ("total", &total.to_string()),
                    ],
                ),
            )
            .await;

            let bot_username = crate::config::bot_username().to_string();
            let caption = tf(
                "youtube.download.caption_part",
                &[
                    ("title", &req.title),
                    ("quality", &quality_label),
                    ("codec", &codec_name),
                    ("bitrate", &bitrate_str),
                    ("sub_tag", sub_tag),
                    ("part", &part_num.to_string()),
                    ("total", &total.to_string()),
                    ("username", &bot_username),
                    ("url", &req.webpage_url),
                ],
            );
            let caption_entities = entities_for_text(&caption);
            let payload = if is_h264 {
                let params = build_part_params(
                    part_path,
                    req.chat_id,
                    &thumb_path,
                    caption,
                    caption_entities,
                    height,
                );
                MediaPayload::Video(params)
            } else {
                let params = build_part_doc_params(
                    part_path,
                    req.chat_id,
                    &thumb_path,
                    caption,
                    caption_entities,
                );
                MediaPayload::Document(params)
            };

            log_trace(
                trace_id,
                "upload_part_start",
                &format!("part={part_num}/{total} path={part_path}"),
            );
            let ok = send_media_with_progress(
                &api,
                payload,
                req.chat_id,
                status_chat_id,
                status_message_id,
                request_id,
                &quality_label,
                &mut cancel_fut,
                trace_id,
            )
            .await;
            if !ok {
                all_ok = false;
                break;
            }
            log_trace(
                trace_id,
                "upload_part_ok",
                &format!("part={part_num}/{total}"),
            );
        }
        all_ok
    } else if is_audio {
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            t("youtube.audio.uploading"),
        )
        .await;
        let bot_username = crate::config::bot_username().to_string();
        let caption = tf(
            "youtube.audio.caption",
            &[
                ("title", &req.title),
                ("quality", &quality_label),
                ("codec", &codec_name),
                ("bitrate", &bitrate_str),
                ("username", &bot_username),
                ("url", &req.webpage_url),
            ],
        );
        let caption_entities = entities_for_text(&caption);
        log_trace(trace_id, "audio_upload_start", &format!("path={path}"));
        send_audio_file(
            &api,
            req.chat_id,
            &path,
            req.title.clone(),
            req.channel.clone(),
            caption,
            caption_entities,
            status_chat_id,
            status_message_id,
            request_id,
            &mut cancel_fut,
            trace_id,
        )
        .await
    } else {
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            t("youtube.download.uploading"),
        )
        .await;
        let bot_username = crate::config::bot_username().to_string();
        let caption = tf(
            "youtube.download.caption",
            &[
                ("title", &req.title),
                ("quality", &quality_label),
                ("codec", &codec_name),
                ("bitrate", &bitrate_str),
                ("sub_tag", sub_tag),
                ("username", &bot_username),
                ("url", &req.webpage_url),
            ],
        );
        let caption_entities = entities_for_text(&caption);
        let payload = if is_h264 {
            let params = build_single_params(
                &path,
                req.chat_id,
                &thumb_path,
                caption,
                caption_entities,
                height,
                req.duration,
            );
            MediaPayload::Video(params)
        } else {
            let params =
                build_single_doc_params(&path, req.chat_id, &thumb_path, caption, caption_entities);
            MediaPayload::Document(params)
        };
        log_trace(trace_id, "upload_start", &format!("path={path}"));
        send_media_with_progress(
            &api,
            payload,
            req.chat_id,
            status_chat_id,
            status_message_id,
            request_id,
            &quality_label,
            &mut cancel_fut,
            trace_id,
        )
        .await
    };

    // In File mode, deliver the standalone subtitle file(s) as documents.
    // (Embedded mode bakes them into the mp4 and needs no separate upload.)
    // Translated subtitles (translated_<lang>.srt) generated in prior translation step
    // are sent alongside remaining subtitle files.
    if upload_ok
        && !is_audio
        && selection.subtitle_mode == SubtitleMode::File
        && !selection.subtitle_langs.is_empty()
    {
        let count = send_subtitle_files(
            &api,
            &dir,
            req.chat_id,
            &req.title,
            &selection.subtitle_langs,
            trace_id,
        )
        .await;
        log_trace(
            trace_id,
            "subtitle_upload_done",
            &format!("files_sent={count}"),
        );
    }

    if upload_ok {
        if !is_audio && !is_h264 {
            maybe_send_non_h264_notice(&api, req.chat_id, &codec_name, trace_id).await;
        }
        if let Some(jid) = stats_job_id {
            stats::record_upload_done(jid, user_id, file_size_bytes as i64, None, Some(1)).await;
        }
        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(status_chat_id)
                    .message_id(status_message_id)
                    .build(),
            )
            .await;
    }

    cleanup_dir(&dir, trace_id).await;
}
