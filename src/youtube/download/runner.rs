use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{DeleteMessageParams, PinChatMessageParams, SendMessageParams, UnpinChatMessageParams},
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Notify;

use crate::i18n::{entities_for_text, t, tf};
use crate::stats;

use super::super::trace::log_trace;
use super::cancel::{register_cancel, unregister_cancel, UnregisterGuard};
use super::helpers::{
    cleanup_dir, fetch_thumbnail, pick_largest_file, quality_label_for, send_subtitle_files,
    sanitize_video_filename, sanitize_audio_filename, maybe_send_non_h264_notice,
};
use super::progress::{format_progress_body, parse_progress_line, ProgressSnapshot};
use super::selection_helpers::find_format;
use super::split::split_video;
use super::status::{edit_progress_status, edit_status};
use super::store::take_request;
use super::types::{Selection, SubtitleMode};
use super::types::AudioQuality;
use super::upload::{
    build_part_doc_params, build_part_params, build_single_doc_params, build_single_params,
    send_audio_file, send_media_with_progress, MediaPayload,
};

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
    tokio::spawn(async move {
        // check اگر playlist است یا single video
        if let Some(req_peek) = super::store::get_request(request_id) {
            if req_peek.is_playlist && !req_peek.playlist_items.is_empty() {
                run_playlist_download(api, request_id, selection, status_chat_id, status_message_id, cancel).await;
                return;
            }
        }
        run_download(api, request_id, selection, status_chat_id, status_message_id, cancel).await
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
        edit_status(&api, status_chat_id, status_message_id, t("youtube.download.request_expired")).await;
        unregister_cancel(request_id);
        return;
    };
    let _cancel_guard = UnregisterGuard(request_id);
    let trace_id = req.trace_id;
    let user_id = req.user_id.unwrap_or(0);
    let stats_job_id = stats::record_download_start(user_id).await;

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
            log_trace(trace_id, "playlist_items_truncated", &format!(
                "user_id={user_id} rank={} original={original_count} limit={limit}",
                user_rank.as_str()
            ));
            let note = if limit == 0 {
                format!("⚠️ دانلود پلی‌لیست برای سطح کاربری شما ({}) غیرمجاز است.", user_rank.as_str())
            } else {
                format!("⚠️ به دلیل محدودیت سطح کاربری ({})، فقط {limit} ویدیوی اول از {original_count} ویدیو دانلود می‌شود.", user_rank.as_str())
            };
            let _ = api.send_message(
                &SendMessageParams::builder()
                    .chat_id(status_chat_id)
                    .text(note)
                    .build()
            ).await;
        }
    }

    let total_videos = req.playlist_items.len();

    log_trace(trace_id, "playlist_download_begin", &format!(
        "total_videos={total_videos} height={} codec={}",
        selection.height, selection.codec.key()
    ));

    // پیام شروع
    let start_msg = tf(
        "youtube.download.playlist.start",
        &[("count", &total_videos.to_string())],
    );
    edit_status(&api, status_chat_id, status_message_id, start_msg).await;

    // پیام وضعیت را پین می‌کنیم تا لای فایل‌های ارسالی گم نشود.
    let _ = api.pin_chat_message(
        &PinChatMessageParams::builder()
            .chat_id(status_chat_id)
            .message_id(status_message_id)
            .disable_notification(true)
            .build(),
    ).await;

    let mut sent = 0usize;
    // (video_num, title, reason) برای هر ویدیویی که ارسال نشد
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

        // ساخت URL برای یک ویدیو
        let video_url = format!("https://www.youtube.com/watch?v={}", item.id);

        // دانلود + ارسال تک ویدیو به پیوی کاربر
        match download_single_playlist_item(
            &api, &video_url, &req, &selection, &item.title, video_num, total_videos, trace_id,
        ).await {
            Ok(bytes) => {
                sent += 1;
                if let Some(job_id) = stats_job_id {
                    stats::record_upload_done(job_id, user_id, bytes as i64).await;
                }
            }
            Err(reason) => {
                log_trace(trace_id, "playlist_item_not_sent", &format!(
                    "num={video_num} id={} reason={reason}", item.id
                ));
                failures.push((video_num, title_short.clone(), reason));
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    // گزارش نهایی: هر ویدیوی ناموفق تک‌تک با دلیل، و در آخر part sent/total
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

    // آنپین پیام وضعیت — کار تمام شده و گزارش نهایی روی همان پیام است.
    let _ = api.unpin_chat_message(
        &UnpinChatMessageParams::builder()
            .chat_id(status_chat_id)
            .message_id(status_message_id)
            .build(),
    ).await;

    log_trace(trace_id, "playlist_download_end", &format!(
        "sent={sent} failed={} total={total_videos}", failures.len()
    ));
    if let Some(job_id) = stats_job_id {
        stats::record_download_done(job_id, (sent as i64) * 100_000_000).await;
    }
}

/// یک ویدیوی پلی‌لیست را در یک پوشه‌ی جدا دانلود، به پیوی کاربر ارسال، و پوشه را پاک می‌کند.
/// `Ok(bytes)` = ارسال موفق؛ `Err(reason)` = دلیل خوانا برای گزارش کاربر.
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

    // یک پوشه‌ی جدا برای هر ویدیو تا فایل‌ها با هم قاطی نشوند
    let dir = PathBuf::from(format!(
        "{}/{trace_id}/pl_{video_num}",
        crate::config::youtube_download_root()
    ));
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        log_trace(trace_id, "playlist_item_mkdir_failed", &format!("url={video_url} err={e}"));
        return Err(e.to_string());
    }

    // به‌جای format_id ثابت (که فقط برای ویدیوی نماینده معتبر است) از انتخاب‌گر
    // مبتنی بر ارتفاع/کدک استفاده می‌کنیم تا هر ویدیو فرمت خودش را انتخاب کند.
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
    cmd.arg("--js-runtimes").arg(format!("deno:{}", crate::config::deno_path()))
        .arg("--cookies-from-browser").arg(&req.cookie_spec)
        .arg("--no-warnings").arg("--no-playlist")
        .arg("-f").arg(&format_spec)
        .arg("--merge-output-format").arg(merge_format)
        .arg("--print").arg("after_move:filepath")
        .arg("-o").arg(&output_template);

    // handling subtitles اگر موجود است
    if !selection.subtitle_langs.is_empty() {
        let sub_langs = selection.subtitle_langs.join(",");
        cmd.arg("--write-subs").arg("--write-auto-subs")
            .arg("--sub-langs").arg(&sub_langs);
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
            log_trace(trace_id, "playlist_item_spawn_failed", &format!("url={video_url} error={e}"));
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
            stderr.lines().rev().find(|l| !l.trim().is_empty())
                .map(|l| {
                    let l = l.trim();
                    if l.chars().count() > 120 { l.chars().take(120).collect::<String>() + "…" } else { l.to_string() }
                })
                .unwrap_or_else(|| t("youtube.download.playlist.reason_no_format"))
        };
        log_trace(trace_id, "playlist_item_download_failed", &format!("url={video_url} reason={reason}"));
        cleanup_dir(&dir, trace_id).await;
        return Err(reason);
    }

    // فایل خروجی: از خط after_move:filepath یا بزرگ‌ترین فایل پوشه
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
        log_trace(trace_id, "playlist_item_no_file", &format!("url={video_url}"));
        cleanup_dir(&dir, trace_id).await;
        return Err(t("youtube.download.playlist.reason_no_file"));
    };

    if selection.subtitle_mode == SubtitleMode::Embedded && !selection.subtitle_langs.is_empty() {
        super::helpers::fix_embedded_subtitle_flags(&path, trace_id).await;
    }

    let file_size_bytes = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);

    // کپشن مثل دانلود تکی
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

    let clean_name = sanitize_video_filename(item_title, &quality_label, &codec_name, &bitrate_str, merge_format);
    let new_path = dir.join(&clean_name);
    let path = if tokio::fs::rename(&path, &new_path).await.is_ok() {
        new_path.to_string_lossy().into_owned()
    } else {
        path
    };
    let bot_username = crate::config::bot_username().to_string();
    let width = total_videos.to_string().len();
    let caption = tf("youtube.download.playlist.caption", &[
        ("title", item_title),
        ("part", &format!("{video_num:0width$}")),
        ("total", &format!("{total_videos:0width$}")),
        ("quality", &quality_label),
        ("codec", &codec_name),
        ("bitrate", &bitrate_str), ("sub_tag", sub_tag),
        ("username", &bot_username),
        ("url", video_url),
    ]);
    let caption_entities = entities_for_text(&caption);
    let send_res = if is_h264 {
        let params = build_single_params(
            &path, req.chat_id, &thumb_path, caption, caption_entities, height, None,
        );
        api.send_video(&params).await.map(|_| ())
    } else {
        let params = build_single_doc_params(
            &path, req.chat_id, &thumb_path, caption, caption_entities,
        );
        api.send_document(&params).await.map(|_| ())
    };

    cleanup_dir(&dir, trace_id).await;
    match send_res {
        Ok(_) => {
            log_trace(trace_id, "playlist_item_upload_ok", &format!("num={video_num}"));
            Ok(file_size_bytes)
        }
        Err(e) => {
            log_trace(trace_id, "playlist_item_upload_failed", &format!("num={video_num} err={e}"));
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
        edit_status(&api, status_chat_id, status_message_id, t("youtube.download.request_expired")).await;
        unregister_cancel(request_id);
        return;
    };
    let _cancel_guard = UnregisterGuard(request_id);
    let mut cancel_fut = std::pin::pin!(cancel.notified());
    let trace_id = req.trace_id;
    let user_id = req.user_id.unwrap_or(0);

    // ثبت شروع دانلود
    let stats_job_id = stats::record_download_start(user_id).await;

    // audio-only path
    if let Some(audio_quality) = selection.audio_only {
        run_audio_download(api, request_id, audio_quality, req, stats_job_id,
            status_chat_id, status_message_id, cancel_fut).await;
        return;
    }

    let quality_label = quality_label_for(height);
    log_trace(trace_id, "download_begin", &format!(
        "request_id={request_id} height={height} codec={} url={}", codec.key(), req.webpage_url
    ));

    let Some(fmt) = find_format(&req, height, codec) else {
        log_trace(trace_id, "download_format_missing", &format!("height={height} codec={}", codec.key()));
        edit_status(&api, status_chat_id, status_message_id,
            tf("youtube.download.failed", &[("error", "format not found")])).await;
        return;
    };
    let format_id = fmt.format_id.clone();

    let dir = PathBuf::from(format!("{}/{trace_id}", crate::config::youtube_download_root()));
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        log_trace(trace_id, "download_mkdir_failed", &e.to_string());
        edit_status(&api, status_chat_id, status_message_id,
            tf("youtube.download.failed", &[("error", &e.to_string())])).await;
        return;
    }

    let output_template = format!("{}/%(id)s.%(ext)s", dir.display());
    let is_h264 = codec == super::super::types::VideoCodec::H264;
    let merge_format = if is_h264 { "mp4" } else { "mkv" };
    let format_spec = match codec {
        super::super::types::VideoCodec::H264 => format!("{format_id}+bestaudio/best"),
        _ => format!("{format_id}+bestaudio/{format_id}/bestvideo[height<={height}]+bestaudio/best"),
    };
    let progress_template = format!(
        "YT_PROGRESS|%(progress._percent_str)s|%(progress._downloaded_bytes_str)s|%(progress._total_bytes_str)s|%(progress._total_bytes_estimate_str)s|%(progress._speed_str)s|%(progress._eta_str)s|%(progress._elapsed_str)s"
    );

    let initial = ProgressSnapshot {
        percent: "0.0%".into(), downloaded: "0B".into(), total: "?".into(),
        speed: "?".into(), eta: "?".into(), elapsed: "00:00".into(), percent_int: 0,
    };
    edit_progress_status(&api, status_chat_id, status_message_id,
        format_progress_body(&initial, &quality_label), request_id).await;

    let postprocess_template = progress_template.clone();
    let mut cmd = tokio::process::Command::new("yt-dlp");
    cmd.arg("--js-runtimes").arg(format!("deno:{}", crate::config::deno_path()))
        .arg("--cookies-from-browser").arg(&req.cookie_spec)
        .arg("--no-warnings").arg("--no-playlist").arg("--progress")
        .arg("--no-color").arg("-f").arg(&format_spec)
        .arg("--merge-output-format").arg(merge_format)
        .arg("--newline")
        .arg("--progress-template").arg(format!("download:{progress_template}"))
        .arg("--progress-template").arg(format!("postprocess:{postprocess_template}"))
        .arg("--print").arg("after_move:filepath")
        .arg("-o").arg(&output_template);

    if !selection.subtitle_langs.is_empty() {
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
        cmd.arg("--write-subs").arg("--write-auto-subs")
            .arg("--sub-langs").arg(&sub_langs)
            .arg("--convert-subs").arg("srt");
        log_trace(trace_id, "download_subtitle_args", &format!(
            "sub_langs={sub_langs} mode={:?} write_auto=true", selection.subtitle_mode
        ));
    }

    cmd.arg(&req.webpage_url).stdout(Stdio::piped()).stderr(Stdio::piped());
    log_trace(trace_id, "download_args", &format!(
        "cookie_spec={} format_spec={format_spec}", req.cookie_spec
    ));

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log_trace(trace_id, "download_spawn_failed", &e.to_string());
            edit_status(&api, status_chat_id, status_message_id,
                tf("youtube.download.failed", &[("error", &e.to_string())])).await;
            return;
        }
    };

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(&'static str, String)>(64);
    let tx_out = tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await { let _ = tx_out.send(("stdout", line)).await; }
    });
    let tx_err = tx;
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await { let _ = tx_err.send(("stderr", line)).await; }
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
            edit_status(&api, status_chat_id, status_message_id,
                tf("youtube.download.failed", &[("error", &e.to_string())])).await;
            return;
        }
    };

    if !status.success() {
        let err = if stderr_tail.is_empty() { format!("exit {status}") } else { stderr_tail };
        log_trace(trace_id, "download_failed", &format!("status={status} err={err}"));
        edit_status(&api, status_chat_id, status_message_id,
            tf("youtube.download.failed", &[("error", &err)])).await;
        cleanup_dir(&dir, trace_id).await;
        return;
    }

    let mut path = match filepath.or_else(|| pick_largest_file(&dir)) {
        Some(p) => p,
        None => {
            log_trace(trace_id, "download_no_filepath", "no output file located");
            edit_status(&api, status_chat_id, status_message_id,
                tf("youtube.download.failed", &[("error", "no output file")])).await;
            cleanup_dir(&dir, trace_id).await;
            return;
        }
    };

    log_trace(trace_id, "download_complete", &format!("path={path}"));

    // مرحله‌ی ترجمه (مستقل از حالت تحویل): اگر کاربر زبانی خواسته که یوتیوب
    // نداشته، انگلیسی را جدا می‌گیریم و به‌صورت محلی (NLLB) ترجمه می‌کنیم.
    // خروجی translated_<lang>.srt در پوشه می‌ماند تا در مرحله‌ی تحویل استفاده شود.
    if !selection.subtitle_langs.is_empty() {
        super::helpers::ensure_translated_subtitles(
            &api, &req.cookie_spec, &req.webpage_url,
            status_chat_id, status_message_id, &dir, &selection.subtitle_langs, trace_id,
        ).await;
    }

    if selection.subtitle_mode == SubtitleMode::Embedded && !selection.subtitle_langs.is_empty() {
        // زیرنویس‌های ترجمه‌شده را هم داخل mp4 می‌گذاریم (yt-dlp فقط زیرنویس اصلی
        // را embed کرده). اگر srt جدیدی نباشد، embed_subtitles همان مسیر را برمی‌گرداند.
        match super::helpers::embed_subtitles(&dir, &path, &selection.subtitle_langs, trace_id).await {
            Ok(new_path) if new_path != path => {
                log_trace(trace_id, "embed_subtitles_remuxed", &format!("path={new_path}"));
                path = new_path;
            }
            Ok(_) => {}
            Err(e) => log_trace(trace_id, "embed_subtitles_failed", &format!("err={e}")),
        }
        super::helpers::fix_embedded_subtitle_flags(&path, trace_id).await;
    } else if selection.subtitle_mode == SubtitleMode::Hardsub && !selection.subtitle_langs.is_empty() {
        match super::helpers::hardsub_subtitles(
            &api, status_chat_id, status_message_id, &dir, &path, &selection.subtitle_langs, req.duration, trace_id, user_id,
        ).await {
            Ok(new_path) if new_path != path => {
                log_trace(trace_id, "hardsub_subtitles_completed", &format!("path={new_path}"));
                path = new_path;
            }
            Ok(_) => {}
            Err(e) => {
                log_trace(trace_id, "hardsub_subtitles_failed", &format!("err={e}"));
                edit_status(&api, status_chat_id, status_message_id, tf("youtube.download.hardsub_failed", &[("error", &e)])).await;
            }
        }
    }

    let file_size_bytes = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);
    if let Some(jid) = stats_job_id {
        stats::record_download_done(jid, file_size_bytes as i64).await;
    }

    let codec_name = t(selection.codec.label_key());
    let bitrate_str = find_format(&req, height, codec)
        .and_then(|f| f.bitrate)
        .map(|b| format!("{:.0}", b))
        .unwrap_or_else(|| "?".to_string());
    let sub_tag = match (selection.subtitle_mode, selection.subtitle_langs.is_empty()) {
        (SubtitleMode::Hardsub, false) => " | Hardsub",
        (SubtitleMode::Embedded, false) => " | Softsub",
        _ => "",
    };
    let thumb_path = fetch_thumbnail(&req.thumbnail_url, &dir, trace_id).await;

    let clean_name = sanitize_video_filename(&req.title, &quality_label, &codec_name, &bitrate_str, merge_format);
    let new_path = dir.join(&clean_name);
    if tokio::fs::rename(&path, &new_path).await.is_ok() {
        log_trace(trace_id, "download_renamed", &format!("old={path} new={}", new_path.display()));
        path = new_path.to_string_lossy().into_owned();
    }

    let file_size_mb = file_size_bytes / (1024 * 1024);
    log_trace(trace_id, "upload_size_check", &format!("size_mb={file_size_mb} max_mb={MAX_SIZE_MB}"));

    let upload_ok = if file_size_mb > MAX_SIZE_MB {
        let num_parts = ((file_size_mb + TARGET_PART_MB - 1) / TARGET_PART_MB) as usize;
        log_trace(trace_id, "split_needed", &format!("size_mb={file_size_mb} parts={num_parts}"));
        edit_status(&api, status_chat_id, status_message_id,
            tf("youtube.download.splitting", &[("parts", &num_parts.to_string())])).await;

        let part_paths = match split_video(&path, &dir, num_parts, req.duration, trace_id).await {
            Ok(p) => p,
            Err(e) => {
                log_trace(trace_id, "split_failed", &e);
                edit_status(&api, status_chat_id, status_message_id,
                    tf("youtube.download.split_failed", &[("error", &e)])).await;
                cleanup_dir(&dir, trace_id).await;
                return;
            }
        };

        let total = part_paths.len();
        let mut all_ok = true;
        for (i, part_path) in part_paths.iter().enumerate() {
            let part_num = i + 1;
            let part_size_mb = tokio::fs::metadata(part_path).await
                .map(|m| m.len() / (1024 * 1024)).unwrap_or(0);
            log_trace(trace_id, "split_part_size", &format!("part={part_num}/{total} size_mb={part_size_mb}"));
            if part_size_mb > MAX_SIZE_MB {
                log_trace(trace_id, "split_part_too_large", &format!("part={part_num} size_mb={part_size_mb}"));
                edit_status(&api, status_chat_id, status_message_id,
                    tf("youtube.download.split_failed", &[("error", &format!("part {part_num} still {part_size_mb}MB"))])).await;
                cleanup_dir(&dir, trace_id).await;
                return;
            }

            edit_status(&api, status_chat_id, status_message_id,
                tf("youtube.download.uploading_part", &[
                    ("part", &part_num.to_string()), ("total", &total.to_string()),
                ])).await;

            let bot_username = crate::config::bot_username().to_string();
            let caption = tf("youtube.download.caption_part", &[
                ("title", &req.title), ("quality", &quality_label),
                ("codec", &codec_name), ("bitrate", &bitrate_str), ("sub_tag", sub_tag),
                ("part", &part_num.to_string()), ("total", &total.to_string()),
                ("username", &bot_username), ("url", &req.webpage_url),
            ]);
            let caption_entities = entities_for_text(&caption);
            let payload = if is_h264 {
                let params = build_part_params(part_path, req.chat_id, &thumb_path,
                    caption, caption_entities, height);
                MediaPayload::Video(params)
            } else {
                let params = build_part_doc_params(part_path, req.chat_id, &thumb_path,
                    caption, caption_entities);
                MediaPayload::Document(params)
            };

            log_trace(trace_id, "upload_part_start", &format!("part={part_num}/{total} path={part_path}"));
            let ok = send_media_with_progress(&api, payload, req.chat_id, status_chat_id,
                status_message_id, request_id, &quality_label, &mut cancel_fut, trace_id).await;
            if !ok { all_ok = false; break; }
            log_trace(trace_id, "upload_part_ok", &format!("part={part_num}/{total}"));
        }
        all_ok
    } else {
        edit_status(&api, status_chat_id, status_message_id, t("youtube.download.uploading")).await;
        let bot_username = crate::config::bot_username().to_string();
        let caption = tf("youtube.download.caption", &[
            ("title", &req.title), ("quality", &quality_label),
            ("codec", &codec_name), ("bitrate", &bitrate_str), ("sub_tag", sub_tag),
            ("username", &bot_username), ("url", &req.webpage_url),
        ]);
        let caption_entities = entities_for_text(&caption);
        let payload = if is_h264 {
            let params = build_single_params(&path, req.chat_id, &thumb_path,
                caption, caption_entities, height, req.duration);
            MediaPayload::Video(params)
        } else {
            let params = build_single_doc_params(&path, req.chat_id, &thumb_path,
                caption, caption_entities);
            MediaPayload::Document(params)
        };
        log_trace(trace_id, "upload_start", &format!("path={path}"));
        send_media_with_progress(&api, payload, req.chat_id, status_chat_id,
            status_message_id, request_id, &quality_label, &mut cancel_fut, trace_id).await
    };

    // In File mode, deliver the standalone subtitle file(s) as documents.
    // (Embedded mode bakes them into the mp4 and needs no separate upload.)
    // زیرنویس‌های ترجمه‌شده (translated_<lang>.srt) در مرحله‌ی ترجمه‌ی پیش از
    // این ساخته شده‌اند و همین‌جا کنار بقیه فرستاده می‌شوند.
    if upload_ok && selection.subtitle_mode == SubtitleMode::File && !selection.subtitle_langs.is_empty() {
        let count = send_subtitle_files(&api, &dir, req.chat_id, &req.title, &selection.subtitle_langs, trace_id).await;
        log_trace(trace_id, "subtitle_upload_done", &format!("files_sent={count}"));
    }

    if upload_ok {
        if !is_h264 {
            maybe_send_non_h264_notice(&api, req.chat_id, &codec_name, trace_id).await;
        }
        if let Some(jid) = stats_job_id {
            stats::record_upload_done(jid, user_id, file_size_bytes as i64).await;
        }
        let _ = api.delete_message(
            &DeleteMessageParams::builder()
                .chat_id(status_chat_id)
                .message_id(status_message_id)
                .build(),
        ).await;
    }

    cleanup_dir(&dir, trace_id).await;
}

async fn run_audio_download(
    api: Bot,
    request_id: u64,
    audio_quality: AudioQuality,
    req: super::types::YoutubeRequest,
    stats_job_id: Option<i64>,
    status_chat_id: i64,
    status_message_id: i32,
    mut cancel_fut: std::pin::Pin<&mut impl std::future::Future<Output = ()>>,
) {
    let trace_id = req.trace_id;
    let user_id = req.user_id.unwrap_or(0);
    log_trace(trace_id, "audio_download_begin", &format!(
        "request_id={request_id} quality={} url={}", audio_quality.as_str(), req.webpage_url
    ));
    let dir = PathBuf::from(format!("{}/{trace_id}", crate::config::youtube_download_root()));
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        log_trace(trace_id, "audio_mkdir_failed", &e.to_string());
        edit_status(&api, status_chat_id, status_message_id,
            tf("youtube.download.failed", &[("error", &e.to_string())])).await;
        return;
    }
    let output_template = format!("{}/%(id)s.%(ext)s", dir.display());
    let format_spec = audio_quality.format_spec();
    let mut cmd = tokio::process::Command::new("yt-dlp");
    cmd.arg("--js-runtimes").arg(format!("deno:{}", crate::config::deno_path()))
        .arg("--cookies-from-browser").arg(&req.cookie_spec)
        .arg("--no-warnings").arg("--no-playlist")
        .arg("-f").arg(format_spec)
        .arg("--extract-audio").arg("--audio-format").arg("mp3").arg("--audio-quality").arg("0")
        .arg("--print").arg("after_move:filepath")
        .arg("-o").arg(&output_template)
        .arg(&req.webpage_url)
        .stdout(Stdio::piped()).stderr(Stdio::piped());
    log_trace(trace_id, "audio_download_args", &format!("cookie_spec={} format_spec={format_spec}", req.cookie_spec));
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log_trace(trace_id, "audio_spawn_failed", &e.to_string());
            edit_status(&api, status_chat_id, status_message_id,
                tf("youtube.download.failed", &[("error", &e.to_string())])).await;
            return;
        }
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(&'static str, String)>(64);
    let tx_out = tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await { let _ = tx_out.send(("stdout", line)).await; }
    });
    let tx_err = tx;
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await { let _ = tx_err.send(("stderr", line)).await; }
    });
    let mut filepath: Option<String> = None;
    let mut stderr_tail = String::new();
    loop {
        tokio::select! {
            msg = rx.recv() => {
                let Some((source, line)) = msg else { break; };
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() { continue; }
                if source == "stdout" && trimmed.starts_with('/') && tokio::fs::metadata(&trimmed).await.is_ok() {
                    filepath = Some(trimmed.clone());
                    log_trace(trace_id, "audio_filepath", &trimmed);
                } else if source == "stderr" {
                    stderr_tail = trimmed.clone();
                    log_trace(trace_id, "audio_yt_dlp_stderr", &trimmed);
                }
            }
            _ = &mut cancel_fut => {
                log_trace(trace_id, "audio_download_cancelled", "cancel signal");
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
            log_trace(trace_id, "audio_wait_failed", &e.to_string());
            edit_status(&api, status_chat_id, status_message_id,
                tf("youtube.download.failed", &[("error", &e.to_string())])).await;
            return;
        }
    };
    if !status.success() {
        let err = if stderr_tail.is_empty() { format!("exit {status}") } else { stderr_tail };
        log_trace(trace_id, "audio_download_failed", &format!("status={status} err={err}"));
        crate::stats::record_error_global("youtube", &format!("audio_download_failed: {err}")).await;
        edit_status(&api, status_chat_id, status_message_id,
            tf("youtube.download.failed", &[("error", &err)])).await;
        cleanup_dir(&dir, trace_id).await;
        return;
    }
    let path = match filepath.or_else(|| pick_largest_file(&dir)) {
        Some(p) => p,
        None => {
            log_trace(trace_id, "audio_no_filepath", "no output file");
            edit_status(&api, status_chat_id, status_message_id,
                tf("youtube.download.failed", &[("error", "no output file")])).await;
            cleanup_dir(&dir, trace_id).await;
            return;
        }
    };
    log_trace(trace_id, "audio_download_complete", &format!("path={path}"));
    let file_size_bytes = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);
    if let Some(jid) = stats_job_id {
        stats::record_download_done(jid, file_size_bytes as i64).await;
    }
    edit_status(&api, status_chat_id, status_message_id, t("youtube.audio.uploading")).await;
    let quality_label = t(audio_quality.label_key());
    let clean_audio_name = sanitize_audio_filename(&req.title, &quality_label, "mp3");
    let new_audio_path = dir.join(&clean_audio_name);
    let path = if tokio::fs::rename(&path, &new_audio_path).await.is_ok() {
        new_audio_path.to_string_lossy().into_owned()
    } else {
        path
    };
    let bot_username = crate::config::bot_username().to_string();
    let caption = tf("youtube.audio.caption", &[
        ("title", &req.title), ("quality", &quality_label), ("username", &bot_username),
        ("url", &req.webpage_url),
    ]);
    let caption_entities = entities_for_text(&caption);
    let upload_ok = send_audio_file(
        &api, req.chat_id, &path, req.title.clone(), req.channel.clone(),
        caption, caption_entities,
        status_chat_id, status_message_id, request_id, &mut cancel_fut, trace_id,
    ).await;
    if upload_ok {
        if let Some(jid) = stats_job_id {
            stats::record_upload_done(jid, user_id, file_size_bytes as i64).await;
        }
        let _ = api.delete_message(
            &DeleteMessageParams::builder()
                .chat_id(status_chat_id).message_id(status_message_id).build(),
        ).await;
    }
    cleanup_dir(&dir, trace_id).await;
}
