use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::DeleteMessageParams,
};
use tokio::sync::Notify;

use crate::i18n::{entities_for_text, t, tf};
use crate::stats;
use crate::youtube::trace::log_trace;
use crate::youtube::types::VideoCodec;

use super::cancel::cancel_guard;
use super::helpers::{
    cleanup_dir, fetch_thumbnail, maybe_send_non_h264_notice, pick_largest_file, quality_label_for,
    sanitize_audio_filename, sanitize_video_filename, send_subtitle_files,
};
use super::progress::{ProgressSnapshot, format_progress_body};
use super::selection_helpers::find_format;
use super::split::split_video;
use super::status::{edit_progress_status, edit_status};
use super::store::take_request;
use super::stream::{YtdlpStreamResult, run_ytdlp_process};
use super::types::{Selection, SubtitleMode};
use super::upload::{
    MediaPayload, build_part_doc_params, build_part_params, build_single_doc_params,
    build_single_params, send_audio_file, send_media_with_progress,
};

const MAX_SIZE_MB: u64 = 2000;
const TARGET_PART_MB: u64 = 1700;

pub(crate) async fn run_download(
    api: Bot,
    request_id: u64,
    selection: Selection,
    status_chat_id: i64,
    status_message_id: i32,
    cancel: Arc<Notify>,
) {
    let _cancel_guard = cancel_guard(request_id);
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
        return;
    };
    let mut cancel_fut = std::pin::pin!(cancel.notified());
    let trace_id = req.trace_id;
    let user_id = req.user_id.unwrap_or(0);

    // Record download start
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

    let stream_res = run_ytdlp_process(
        cmd,
        &api,
        status_chat_id,
        status_message_id,
        request_id,
        &quality_label,
        &mut cancel_fut,
        trace_id,
        &dir,
    )
    .await;

    let (filepath, stderr_tail, status) = match stream_res {
        YtdlpStreamResult::Completed {
            filepath,
            stderr_tail,
            status,
        } => (filepath, stderr_tail, status),
        YtdlpStreamResult::Cancelled | YtdlpStreamResult::Failed => return,
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
