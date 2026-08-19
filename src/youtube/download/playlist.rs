use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{PinChatMessageParams, SendMessageParams, UnpinChatMessageParams},
};
use tokio::sync::Notify;

use crate::i18n::{entities_for_text, t, tf};
use crate::stats;
use crate::youtube::trace::log_trace;

use super::cancel::cancel_guard;
use super::helpers::{
    cleanup_dir, fetch_thumbnail, maybe_send_non_h264_notice, pick_largest_file, quality_label_for,
    sanitize_video_filename,
};
use super::selection_helpers::find_format;
use super::status::edit_status;
use super::types::{Selection, SubtitleMode, YoutubeRequest};
use super::upload::{build_single_doc_params, build_single_params};

pub(crate) async fn run_playlist_download(
    api: Bot,
    request_id: u64,
    selection: Selection,
    status_chat_id: i64,
    status_message_id: i32,
    _cancel: Arc<Notify>,
) {
    let _cancel_guard = cancel_guard(request_id);
    let Some(mut req) = super::store::take_request(request_id) else {
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            t("youtube.download.request_expired"),
        )
        .await;
        return;
    };
    let trace_id = req.trace_id;
    let user_id = req.user_id.unwrap_or(0);
    let stats_job_id = stats::record_download_start(user_id, "youtube").await;
    let _active_dl_guard = crate::metrics::ActiveDownloadGuard::new();
    let _duration_guard = crate::metrics::RequestDurationGuard::new("youtube");

    let user_rank = if user_id > 0 {
        if let Some(pool) = stats::get_pool() {
            if let Ok(client) = pool.get().await {
                crate::rank::effective_rank(&client, user_id).await
            } else {
                crate::rank::types::Rank::Dalavar
            }
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
    req: &YoutubeRequest,
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
