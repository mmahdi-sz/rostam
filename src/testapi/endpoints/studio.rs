use axum::Json;
use serde::{Deserialize, Serialize};

use crate::i18n::{apply_premium_to_html, apply_premium_to_md, md_escape, t, tf};
use crate::studio::is_telegram_audio_guaranteed;
use crate::studio::trim::{
    DEFAULT_MAX_CUT_RANGES, RangeError, cancel_keyboard, format_timestamp, parse_cut_ranges,
};

#[derive(Deserialize)]
pub struct StudioTrimReq {
    #[allow(dead_code)]
    pub user_id: Option<i64>,
    pub input_ranges: Option<String>,
    pub duration_secs: Option<u64>,
    pub media_kind: Option<String>,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_bitrate: Option<u64>,
    pub simulate_busy: Option<bool>,
    pub simulate_paywall: Option<bool>,
    pub simulate_quota_exhausted: Option<bool>,
}

#[derive(Serialize)]
pub struct ParsedRangeDto {
    pub start_secs: u64,
    pub end_secs: u64,
    pub raw_line: String,
}

#[derive(Serialize)]
pub struct ButtonDto {
    pub text: String,
    pub callback_data: String,
    pub style: String,
}

#[derive(Serialize)]
pub struct StudioTrimResp {
    pub ok: bool,
    pub is_valid: bool,
    pub ranges_count: usize,
    pub parsed_ranges: Vec<ParsedRangeDto>,
    pub prompt_text: String,
    pub prompt_keyboard: Vec<Vec<ButtonDto>>,
    pub info_template: String,
    pub status_job_ticker: String,
    pub not_a_video_err_sample: String,
    pub errors: Vec<String>,

    pub detected_media_kind: String,
    pub is_audio: bool,
    pub delivery_method: String,
    pub rendered_info_card: String,
    pub sample_ffmpeg_args: Vec<String>,
    pub busy_blocked: bool,
    pub busy_error_text: Option<String>,
    pub paywall_blocked: bool,
    pub paywall_error_text: Option<String>,
    pub quota_blocked: bool,
    pub quota_error_text: Option<String>,
    pub stats_events: Vec<String>,
    pub trace: u64,
}

fn dump(kbd: &frankenstein::types::InlineKeyboardMarkup) -> Vec<Vec<ButtonDto>> {
    kbd.inline_keyboard
        .iter()
        .map(|row| {
            row.iter()
                .map(|b| ButtonDto {
                    text: b.text.clone(),
                    callback_data: b.callback_data.clone().unwrap_or_default(),
                    style: match b.style {
                        Some(frankenstein::types::ButtonStyle::Success) => "success",
                        Some(frankenstein::types::ButtonStyle::Primary) => "primary",
                        Some(frankenstein::types::ButtonStyle::Danger) => "danger",
                        _ => "default",
                    }
                    .to_string(),
                })
                .collect()
        })
        .collect()
}

pub async fn test_studio_trim(
    Json(req): Json<StudioTrimReq>,
) -> (axum::http::StatusCode, Json<StudioTrimResp>) {
    let input = req
        .input_ranges
        .unwrap_or_else(|| "00:00 - 00:30\n۰۰:۰۱:۰۰ - ۰۰:۰۲:۰۰".to_string());
    let duration_secs = req.duration_secs.unwrap_or(300);

    let parsed_res = parse_cut_ranges(&input, duration_secs, DEFAULT_MAX_CUT_RANGES);
    let mut is_valid = true;
    let mut parsed_dtos = Vec::new();
    let mut err_strings = Vec::new();

    match parsed_res {
        Ok(ranges) => {
            for r in ranges {
                parsed_dtos.push(ParsedRangeDto {
                    start_secs: r.start_secs,
                    end_secs: r.end_secs,
                    raw_line: r.raw_line,
                });
            }
        }
        Err(errors) => {
            is_valid = false;
            for err in errors {
                match err {
                    RangeError::InvalidFormat { line_idx, text } => {
                        err_strings.push(tf(
                            "studio.trim.error.invalid_format",
                            &[("line", &line_idx.to_string()), ("text", &md_escape(&text))],
                        ));
                    }
                    RangeError::StartGteEnd { line_idx, .. } => {
                        err_strings.push(tf(
                            "studio.trim.error.start_gte_end",
                            &[("line", &line_idx.to_string())],
                        ));
                    }
                    RangeError::EndExceedsDuration {
                        line_idx,
                        end,
                        duration,
                    } => {
                        err_strings.push(tf(
                            "studio.trim.error.end_exceeds_duration",
                            &[
                                ("line", &line_idx.to_string()),
                                ("end", &md_escape(&format_timestamp(end))),
                                ("duration", &md_escape(&format_timestamp(duration))),
                            ],
                        ));
                    }
                    RangeError::ExceedsMaxRanges { max } => {
                        err_strings.push(tf(
                            "studio.trim.error.max_ranges",
                            &[("max", &max.to_string())],
                        ));
                    }
                    RangeError::NoValidRanges => {
                        err_strings.push(t("studio.trim.error.no_valid_ranges"));
                    }
                }
            }
        }
    }

    let detected_media_kind = if let Some(ref k) = req.media_kind {
        match k.to_lowercase().as_str() {
            "audio" | "voice" | "music" => "audio".to_string(),
            "unsupported" | "document" | "pdf" | "archive" => "unsupported".to_string(),
            "video" => "video".to_string(),
            _ => "video".to_string(),
        }
    } else if let Some(ref mime) = req.mime_type {
        let m = mime.to_lowercase();
        if m.starts_with("audio/") {
            "audio".to_string()
        } else if m.starts_with("video/") {
            "video".to_string()
        } else if m == "application/pdf"
            || m == "application/zip"
            || m == "application/x-rar-compressed"
            || m == "application/x-7z-compressed"
        {
            "unsupported".to_string()
        } else {
            "video".to_string()
        }
    } else if let Some(ref name) = req.filename {
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        if matches!(
            ext.as_str(),
            "mp3" | "wav" | "flac" | "m4a" | "ogg" | "opus" | "aac"
        ) {
            "audio".to_string()
        } else if matches!(
            ext.as_str(),
            "mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "wmv" | "m4v"
        ) {
            "video".to_string()
        } else if matches!(ext.as_str(), "pdf" | "zip" | "rar" | "7z" | "txt" | "exe") {
            "unsupported".to_string()
        } else {
            "video".to_string()
        }
    } else {
        "video".to_string()
    };

    if detected_media_kind == "unsupported" {
        is_valid = false;
        err_strings.push(t("studio.trim.error.not_a_video"));
    }

    let is_audio = detected_media_kind == "audio";
    let delivery_method = if is_audio {
        let codec = req.audio_codec.as_deref().unwrap_or("mp3");
        let ext = req
            .filename
            .as_deref()
            .and_then(|f| f.rsplit('.').next())
            .unwrap_or(codec);
        if is_telegram_audio_guaranteed(ext) {
            "sendAudio".to_string()
        } else {
            "sendDocument".to_string()
        }
    } else {
        "sendVideo".to_string()
    };

    let prompt_text = apply_premium_to_md(&t("studio.trim.ranges_prompt"));
    let prompt_keyboard = dump(&cancel_keyboard());

    let raw_info = tf(
        "studio.trim.info_template",
        &[
            ("filename", &md_escape("test.mp4")),
            ("width", "1920"),
            ("height", "1080"),
            ("bitrate", &md_escape("2500 kbps")),
            ("fps", "30"),
            ("codec", &md_escape("h264")),
            ("duration", &md_escape(&format_timestamp(duration_secs))),
        ],
    );
    let info_template = apply_premium_to_md(&raw_info);

    let rendered_info_card = if is_audio {
        let audio_filename = req.filename.as_deref().unwrap_or("track.mp3");
        let audio_codec = req.audio_codec.as_deref().unwrap_or("mp3");
        let bitrate_str = req
            .audio_bitrate
            .map(|b| format!("{} kbps", b / 1000))
            .unwrap_or_else(|| "320 kbps".to_string());
        let raw_audio = tf(
            "studio.trim.info_audio_template",
            &[
                ("filename", &md_escape(audio_filename)),
                ("duration", &md_escape(&format_timestamp(duration_secs))),
                ("bitrate", &md_escape(&bitrate_str)),
                ("codec", &md_escape(audio_codec)),
            ],
        );
        apply_premium_to_md(&raw_audio)
    } else {
        let video_filename = req.filename.as_deref().unwrap_or("test.mp4");
        let raw_video = tf(
            "studio.trim.info_template",
            &[
                ("filename", &md_escape(video_filename)),
                ("width", "1920"),
                ("height", "1080"),
                ("bitrate", &md_escape("2500 kbps")),
                ("fps", "30"),
                ("codec", &md_escape("h264")),
                ("duration", &md_escape(&format_timestamp(duration_secs))),
            ],
        );
        apply_premium_to_md(&raw_video)
    };

    let sample_ffmpeg_args = if is_audio {
        let codec = req.audio_codec.as_deref().unwrap_or("mp3");
        if codec == "flac" {
            vec![
                "-y".to_string(),
                "-ss".to_string(),
                "00:00:00".to_string(),
                "-to".to_string(),
                "00:01:00".to_string(),
                "-i".to_string(),
                "input.flac".to_string(),
                "-map".to_string(),
                "0".to_string(),
                "-c:a".to_string(),
                "flac".to_string(),
            ]
        } else if codec == "wav" {
            vec![
                "-y".to_string(),
                "-ss".to_string(),
                "00:00:00".to_string(),
                "-to".to_string(),
                "00:01:00".to_string(),
                "-i".to_string(),
                "input.wav".to_string(),
                "-map".to_string(),
                "0".to_string(),
                "-c".to_string(),
                "copy".to_string(),
                "-avoid_negative_ts".to_string(),
                "make_zero".to_string(),
            ]
        } else {
            vec![
                "-y".to_string(),
                "-ss".to_string(),
                "00:00:00".to_string(),
                "-to".to_string(),
                "00:01:00".to_string(),
                "-i".to_string(),
                "input.mp3".to_string(),
                "-map".to_string(),
                "0".to_string(),
                "-c".to_string(),
                "copy".to_string(),
            ]
        }
    } else {
        vec![
            "-y".to_string(),
            "-ss".to_string(),
            "00:00:00".to_string(),
            "-to".to_string(),
            "00:01:00".to_string(),
            "-i".to_string(),
            "input.mp4".to_string(),
            "-c".to_string(),
            "copy".to_string(),
            "-avoid_negative_ts".to_string(),
            "make_zero".to_string(),
            "-movflags".to_string(),
            "+faststart".to_string(),
        ]
    };

    let busy_blocked = req.simulate_busy.unwrap_or(false);
    let busy_error_text = if busy_blocked {
        Some(t("active_job_running"))
    } else {
        None
    };

    let paywall_blocked = req.simulate_paywall.unwrap_or(false);
    let paywall_error_text = if paywall_blocked {
        Some(apply_premium_to_html(&tf(
            "rank.paywall_feature",
            &[("feature", "studio_trim"), ("min_rank", "سپهبد")],
        )))
    } else {
        None
    };

    let quota_blocked = req.simulate_quota_exhausted.unwrap_or(false);
    let quota_error_text = if quota_blocked {
        Some(apply_premium_to_html(&tf(
            "rank.paywall_limit",
            &[("limit", "10"), ("min_rank", "سپهبد")],
        )))
    } else {
        None
    };

    let trace = crate::log::next_trace_id();
    let mut stats_events = Vec::new();
    if paywall_blocked {
        stats_events.push("paywall:feature:sepahbod".to_string());
    } else if quota_blocked {
        stats_events.push("paywall:limit:sepahbod".to_string());
    } else if busy_blocked {
        stats_events.push("studio_trim:busy_blocked".to_string());
    } else if detected_media_kind == "unsupported" {
        stats_events.push("studio_trim:invalid_media".to_string());
    } else {
        stats_events.push("studio_trim:ranges_parsed".to_string());
        stats_events.push(format!("studio_trim:media:{detected_media_kind}"));
    }

    let raw_ticker = tf(
        "studio.trim.status_job_ticker",
        &[
            ("current", "1"),
            ("total", "2"),
            ("elapsed", &md_escape("5s")),
        ],
    );
    let status_job_ticker = apply_premium_to_md(&raw_ticker);

    let not_a_video_err_sample = apply_premium_to_md(&t("studio.trim.error.not_a_video"));

    (
        axum::http::StatusCode::OK,
        Json(StudioTrimResp {
            ok: true,
            is_valid,
            ranges_count: parsed_dtos.len(),
            parsed_ranges: parsed_dtos,
            prompt_text,
            prompt_keyboard,
            info_template,
            status_job_ticker,
            not_a_video_err_sample,
            errors: err_strings,
            detected_media_kind,
            is_audio,
            delivery_method,
            rendered_info_card,
            sample_ffmpeg_args,
            busy_blocked,
            busy_error_text,
            paywall_blocked,
            paywall_error_text,
            quota_blocked,
            quota_error_text,
            stats_events,
            trace,
        }),
    )
}

#[derive(Deserialize)]
pub struct StudioCompressReq {
    pub orig_w: Option<u32>,
    pub orig_h: Option<u32>,
    pub orig_fps: Option<u32>,
    pub orig_bitrate: Option<u64>,
    pub duration_secs: Option<u64>,
    pub selected_codec: Option<String>,
    pub selected_res: Option<u32>,
    pub selected_fps: Option<u32>,
    pub selected_br_ratio: Option<u32>,
}

#[derive(Serialize)]
pub struct StudioCompressResp {
    pub ok: bool,
    pub menu_text: String,
    pub menu_keyboard: Vec<Vec<ButtonDto>>,
    pub available_resolutions: Vec<u32>,
    pub available_fps: Vec<u32>,
    pub selected_codec: String,
    pub selected_res: u32,
    pub selected_fps: u32,
    pub selected_br_ratio: u32,
    pub calculated_target_bitrate_kbps: u64,
    pub estimated_size_mb: f64,
    pub container: String,
    pub preset: String,
    pub status_job_ticker: String,
    pub status_job_ticker_eta: String,
    pub job_done_sample: String,
    pub output_filename: String,
    pub not_a_video_err_sample: String,
}

pub async fn test_studio_compress(
    Json(req): Json<StudioCompressReq>,
) -> (axum::http::StatusCode, Json<StudioCompressResp>) {
    use crate::studio::compress::{
        CompressSession, build_compress_keyboard, build_compress_text, calculate_estimated_size_mb,
        calculate_target_bitrate_kbps, format_eta_hms,
    };

    let orig_w = req.orig_w.unwrap_or(1920);
    let orig_h = req.orig_h.unwrap_or(1080);
    let orig_fps = req.orig_fps.unwrap_or(60);
    let orig_bitrate = req.orig_bitrate.unwrap_or(2_000_000);
    let duration_secs = req.duration_secs.unwrap_or(120);

    let base_dim = orig_w.min(orig_h);
    let res_h = req.selected_res.unwrap_or(1080).min(base_dim);
    let fps = req.selected_fps.unwrap_or(60).min(orig_fps);
    let codec = req.selected_codec.unwrap_or_else(|| "h264".to_string());
    let br_ratio = req.selected_br_ratio.unwrap_or(100);

    let session = CompressSession {
        file_id: "test_file_id".to_string(),
        filename: "test_video.mp4".to_string(),
        orig_w,
        orig_h,
        orig_fps,
        orig_bitrate,
        orig_codec: "h264".to_string(),
        orig_size_bytes: 30_000_000,
        duration_secs,
        codec: codec.clone(),
        res_h,
        fps,
        br_ratio,
    };

    let menu_text = build_compress_text(&session);
    let menu_keyboard = dump(&build_compress_keyboard(&session));

    let all_res = [2160, 1440, 1080, 720, 480, 360, 240, 144];
    let available_resolutions: Vec<u32> = all_res.into_iter().filter(|&h| h <= base_dim).collect();

    let all_fps = [60, 45, 30, 24, 20, 15, 13];
    let available_fps: Vec<u32> = all_fps.into_iter().filter(|&f| f <= orig_fps).collect();

    let calculated_target_bitrate_kbps = calculate_target_bitrate_kbps(&session, res_h, br_ratio);
    let estimated_size_mb = calculate_estimated_size_mb(&session, res_h, br_ratio);
    let container = if codec == "h264" {
        ".mp4".to_string()
    } else {
        ".mkv".to_string()
    };

    let preset = if codec == "av1" {
        "9".to_string()
    } else {
        "medium".to_string()
    };

    let elapsed_sample = format_eta_hms(187); // 3 دقیقه و 7 ثانیه
    let eta_sample = format_eta_hms(707); // 11 دقیقه و 47 ثانیه

    let status_job_ticker = apply_premium_to_md(&tf(
        "studio.compress.status_job_ticker",
        &[("elapsed", &md_escape(&elapsed_sample)), ("eta", "")],
    ));

    let eta_part = tf(
        "studio.compress.status_job_ticker_eta",
        &[("eta", &md_escape(&eta_sample))],
    );
    let status_job_ticker_eta = apply_premium_to_md(&tf(
        "studio.compress.status_job_ticker",
        &[("elapsed", &md_escape(&elapsed_sample)), ("eta", &eta_part)],
    ));

    let metadata = crate::studio::trim::probe::VideoMetadata {
        filename: session.filename.clone(),
        width: 854,
        height: 480,
        bitrate: 1_300_000,
        stream_bitrate: Some(1_234_000),
        fps: 30,
        average_fps: Some(29.97),
        codec: codec.clone(),
        duration_secs,
    };
    use crate::studio::compress::runner::{output_details, output_filename};
    let output_filename = output_filename("test_video", &container[1..], &metadata, None);
    let done = tf(
        "studio.compress.job_done",
        &[
            ("filename", &md_escape(&output_filename)),
            ("orig_size", &md_escape("903.4")),
            ("final_size", &md_escape("597.2")),
            ("saved_percent", "34"),
            ("compress_time", &md_escape(&format_eta_hms(1293))),
            ("download_time", &md_escape(&format_eta_hms(0))),
            ("vmaf_score", &md_escape("94.23")),
        ],
    );

    let job_done_sample = apply_premium_to_md(&format!("{done}{}", output_details(&metadata)));

    let not_a_video_err_sample = apply_premium_to_md(&t("studio.compress.error.not_a_video"));

    (
        axum::http::StatusCode::OK,
        Json(StudioCompressResp {
            ok: true,
            menu_text,
            menu_keyboard,
            available_resolutions,
            available_fps,
            selected_codec: codec,
            selected_res: res_h,
            selected_fps: fps,
            selected_br_ratio: br_ratio,
            calculated_target_bitrate_kbps,
            estimated_size_mb,
            container,
            preset,
            status_job_ticker,
            status_job_ticker_eta,
            job_done_sample,
            output_filename,
            not_a_video_err_sample,
        }),
    )
}

#[derive(Deserialize)]
pub struct TestStreamEntryDto {
    pub kind: String,
    pub codec_name: String,
    #[allow(dead_code)]
    pub language: Option<String>,
}

#[derive(Deserialize)]
pub struct StudioExtractReq {
    pub streams: Option<Vec<TestStreamEntryDto>>,
}

#[derive(Serialize)]
pub struct StudioExtractResp {
    pub ok: bool,
    pub total_streams: usize,
    pub audio_count: usize,
    pub sub_count: usize,
    pub mapped_extensions: Vec<String>,
    pub prompt_text: String,
    pub prompt_keyboard: Vec<Vec<ButtonDto>>,
    pub status_downloading: String,
    pub status_probing: String,
    pub status_extracting: String,
    pub status_uploading: String,
    pub job_done_rendered_text: String,
    pub no_streams_err_rendered_text: String,
}

pub async fn test_studio_extract(
    Json(req): Json<StudioExtractReq>,
) -> (axum::http::StatusCode, Json<StudioExtractResp>) {
    use crate::studio::extract::{StreamKind, cancel_keyboard, map_codec_to_ext};

    let sample_streams = req.streams.unwrap_or_else(|| {
        vec![
            TestStreamEntryDto {
                kind: "audio".to_string(),
                codec_name: "aac".to_string(),
                language: Some("eng".to_string()),
            },
            TestStreamEntryDto {
                kind: "subtitle".to_string(),
                codec_name: "subrip".to_string(),
                language: Some("fas".to_string()),
            },
        ]
    });

    let mut audio_count = 0usize;
    let mut sub_count = 0usize;
    let mut mapped_extensions = Vec::new();

    for st in &sample_streams {
        let kind = if st.kind.to_lowercase() == "subtitle" {
            sub_count += 1;
            StreamKind::Subtitle
        } else {
            audio_count += 1;
            StreamKind::Audio
        };
        let ext = map_codec_to_ext(&kind, &st.codec_name);
        mapped_extensions.push(ext.to_string());
    }

    let total_streams = audio_count + sub_count;

    let prompt_text = apply_premium_to_md(&t("studio.extract.send_video_prompt"));
    let prompt_keyboard = dump(&cancel_keyboard());

    let status_downloading = apply_premium_to_md(&tf(
        "studio.extract.status_downloading",
        &[("elapsed", "0s"), ("detail", "")],
    ));
    let status_probing = apply_premium_to_md(&t("studio.extract.status_probing"));
    let status_extracting = apply_premium_to_md(&tf(
        "studio.extract.status_extracting",
        &[("elapsed", &md_escape("12s"))],
    ));
    let status_uploading = apply_premium_to_md(&tf(
        "studio.extract.status_uploading",
        &[
            ("current", "1"),
            ("total", &total_streams.to_string()),
            ("filename", &md_escape("sample_audio_1_eng.m4a")),
        ],
    ));

    let job_done_rendered_text = apply_premium_to_md(&tf(
        "studio.extract.job_done",
        &[
            ("audio_count", &audio_count.to_string()),
            ("sub_count", &sub_count.to_string()),
            ("total_time", &md_escape("00:15")),
        ],
    ));

    let no_streams_err_rendered_text = apply_premium_to_md(&t("studio.extract.error.no_streams"));

    (
        axum::http::StatusCode::OK,
        Json(StudioExtractResp {
            ok: true,
            total_streams,
            audio_count,
            sub_count,
            mapped_extensions,
            prompt_text,
            prompt_keyboard,
            status_downloading,
            status_probing,
            status_extracting,
            status_uploading,
            job_done_rendered_text,
            no_streams_err_rendered_text,
        }),
    )
}

#[derive(Deserialize)]
pub struct StudioBurnReq {
    pub sub_filename: Option<String>,
    pub order: Option<String>,
    /// Raw video filename as Telegram would report it (used for the sanitize check).
    pub video_filename: Option<String>,
    /// Probed duration in seconds, to exercise the `too_long` cap.
    pub duration_secs: Option<u64>,
    /// Probed source video codec, to exercise encoder matching (`av1`, `hevc`, `vp9`, …).
    pub source_codec: Option<String>,
    /// Size of the burned output, to exercise the oversized-output split path.
    pub output_bytes: Option<u64>,
}

#[derive(Serialize)]
pub struct StudioBurnResp {
    pub ok: bool,
    pub sub_format: String,
    pub filter_type: String,
    pub filter_arg: String,
    pub order_used: String,
    /// What `handle_input_message` would do with this document: subtitle | video | unsupported.
    pub route_decision: String,
    /// Fixed in-work-dir names — no user string ever reaches the path or the filtergraph.
    pub sub_workdir_name: String,
    pub video_workdir_name: String,
    /// Caption-only display name.
    pub sanitized_display_name: String,
    pub max_duration_secs: u64,
    pub duration_blocked: bool,
    /// Source codec as probed, and the encoder the burn re-encodes with. Re-encoding an AV1
    /// source with libx264 inflates the output past the 2000 MB upload cap.
    pub source_codec: String,
    pub video_encoder: String,
    pub video_encoder_args: Vec<String>,
    /// Oversized output is split instead of rejected. `split_needed` false => sent as one file.
    pub max_upload_bytes: u64,
    pub split_needed: bool,
    pub split_parts_planned: u64,
    /// Largest a piece can be once split — must stay under `max_upload_bytes`.
    pub split_part_bytes_max: u64,
    pub split_segment_secs: u64,
    pub status_splitting_text: String,
    pub job_done_part_rendered_text: String,
    pub prompt_text: String,
    pub prompt_keyboard: Vec<Vec<ButtonDto>>,
    pub job_cancel_keyboard: Vec<Vec<ButtonDto>>,
    pub video_received_text: String,
    pub sub_received_text: String,
    pub sub_replaced_text: String,
    pub status_downloading_text: String,
    pub status_burning_text: String,
    pub status_uploading_text: String,
    pub job_done_rendered_text: String,
    pub job_cancelled_text: String,
    pub unsupported_sub_err_text: String,
    pub too_long_err_text: String,
    pub download_failed_err_text: String,
    pub oversized_err_text: String,
    pub burn_failed_err_text: String,
    pub stats_events: Vec<String>,
    pub trace: u64,
}

pub async fn test_studio_burn(
    Json(req): Json<StudioBurnReq>,
) -> (axum::http::StatusCode, Json<StudioBurnResp>) {
    let sub_filename = req.sub_filename.unwrap_or_else(|| "sub.ass".to_string());
    let order = req.order.unwrap_or_else(|| "video_first".to_string());
    let video_filename = req
        .video_filename
        .unwrap_or_else(|| "../../etc/passwd.mp4".to_string());
    let duration_secs = req.duration_secs.unwrap_or(120);
    let trace = crate::log::next_trace_id();

    let fmt = crate::studio::burn::detect_subtitle_format(&sub_filename);
    let sub_format = match fmt {
        Some(crate::studio::burn::SubtitleFormat::Ass) => "ass",
        Some(crate::studio::burn::SubtitleFormat::Srt) => "srt",
        Some(crate::studio::burn::SubtitleFormat::Vtt) => "vtt",
        None => "unsupported",
    };

    // Real routing order from `handle_input_message`: subtitle extension wins over the
    // permissive video-metadata guess, which is why a `.srt` document is no longer burned as video.
    let doc_msg: frankenstein::types::Message = serde_json::from_str(&format!(
        r#"{{"message_id":1,"date":1000,"chat":{{"id":1,"type":"private"}},
             "document":{{"file_id":"d1","file_unique_id":"u1","file_name":{},
             "mime_type":"application/octet-stream","file_size":100}}}}"#,
        serde_json::to_string(&sub_filename).unwrap_or_else(|_| "\"x\"".into())
    ))
    .expect("static test message json");
    let route_decision = if fmt.is_some() {
        "subtitle"
    } else if crate::studio::is_video_message_metadata(&doc_msg) {
        "video"
    } else {
        "unsupported"
    }
    .to_string();

    let sub_workdir_name = match fmt {
        Some(f) => format!("sub.{}", f.ext()),
        None => String::new(),
    };
    let sanitized_display_name = {
        let c = crate::validation::sanitize_filename(&video_filename);
        if c.is_empty() {
            "video.mp4".to_string()
        } else {
            c
        }
    };
    let video_ext = std::path::Path::new(&sanitized_display_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4")
        .to_lowercase();
    let video_workdir_name = format!("input.{video_ext}");

    let sample_path = std::path::Path::new("/tmp/test_workdir").join(&sub_workdir_name);
    let (filter_type, filter_arg) = match fmt {
        Some(f) => (
            match f {
                crate::studio::burn::SubtitleFormat::Ass => "ass",
                _ => "subtitles",
            }
            .to_string(),
            crate::studio::burn::build_filter_arg(f, &sample_path),
        ),
        None => ("none".to_string(), String::new()),
    };

    let prompt_text = apply_premium_to_md(&t("studio.burn.prompt"));
    let prompt_keyboard = dump(&crate::studio::burn::cancel_keyboard());
    let job_cancel_keyboard = dump(&crate::studio::burn::job_cancel_keyboard());

    let video_received_text = apply_premium_to_md(&t("studio.burn.video_received_need_sub"));
    let sub_received_text = apply_premium_to_md(&t("studio.burn.sub_received_need_video"));
    let sub_replaced_text = apply_premium_to_md(&t("studio.burn.sub_replaced"));

    let status_downloading_text = apply_premium_to_md(&tf(
        "studio.burn.status_downloading",
        &[
            ("elapsed", &md_escape("4s")),
            (
                "detail",
                &tf(
                    "studio.burn.status_downloading_detail",
                    &[
                        ("dl_mb", &md_escape("12.0")),
                        ("total_mb", &md_escape("48.0")),
                        ("pct", "25"),
                        ("speed", &md_escape("3.0 MB/s")),
                        ("eta", &md_escape("12s")),
                    ],
                ),
            ),
        ],
    ));

    let status_burning_text = apply_premium_to_md(&tf(
        "studio.burn.status_burning",
        &[
            ("elapsed", &md_escape("10s")),
            ("pct", "45"),
            ("speed", &md_escape("1.2x")),
            ("eta", &md_escape("12s")),
        ],
    ));

    let status_uploading_text = apply_premium_to_md(&t("studio.burn.status_uploading"));

    let job_done_rendered_text = apply_premium_to_md(&tf(
        "studio.burn.job_done",
        &[
            ("filename", &md_escape("burned_sample.mp4")),
            ("burn_time", &md_escape("22s")),
        ],
    ));

    let job_cancelled_text = apply_premium_to_md(&t("studio.burn.job_cancelled"));
    let unsupported_sub_err_text = apply_premium_to_md(&t("studio.burn.error.unsupported_sub"));
    let max_duration_secs = crate::studio::burn::MAX_BURN_DURATION_SECS;
    let duration_blocked = duration_secs > max_duration_secs;
    let source_codec = req.source_codec.clone().unwrap_or_else(|| "h264".into());
    let enc_args = crate::studio::burn::video_encoder_args(&source_codec);
    let video_encoder = enc_args.get(1).copied().unwrap_or("").to_string();
    let video_encoder_args: Vec<String> = enc_args.iter().map(|s| s.to_string()).collect();

    let max_upload_bytes = crate::studio::burn::MAX_UPLOAD_BYTES;
    let output_bytes = req.output_bytes.unwrap_or(100 * 1024 * 1024);
    let split_needed = output_bytes > max_upload_bytes;
    let split_parts_planned = if split_needed {
        crate::studio::burn::upload_part_count(output_bytes, max_upload_bytes)
    } else {
        1
    };
    let split_part_bytes_max = output_bytes.div_ceil(split_parts_planned.max(1));
    let split_segment_secs =
        crate::studio::burn::split_segment_secs(duration_secs, split_parts_planned);
    let status_splitting_text = apply_premium_to_md(&tf(
        "studio.burn.status_splitting",
        &[("parts", &md_escape(&split_parts_planned.to_string()))],
    ));
    let job_done_part_rendered_text = apply_premium_to_md(&tf(
        "studio.burn.job_done_part",
        &[
            ("filename", &md_escape("burned_sample.mp4")),
            ("burn_time", &md_escape("22s")),
            ("part", &md_escape("1")),
            ("total", &md_escape(&split_parts_planned.to_string())),
        ],
    ));
    let too_long_err_text = apply_premium_to_md(&tf(
        "studio.burn.error.too_long",
        &[
            (
                "duration",
                &md_escape(&crate::studio::compress::format_eta_hms(duration_secs)),
            ),
            (
                "max",
                &md_escape(&crate::studio::compress::format_eta_hms(max_duration_secs)),
            ),
        ],
    ));
    let download_failed_err_text = apply_premium_to_md(&t("studio.burn.error.download_failed"));
    let oversized_err_text = apply_premium_to_md(&t("studio.burn.error.oversized"));
    let burn_failed_err_text = apply_premium_to_md(&t("studio.burn.error.burn_failed"));

    let mut stats_events = vec!["studio_burn/burn/start".to_string()];
    if route_decision == "unsupported" {
        stats_events.push("studio_burn/burn/unsupported_sub".to_string());
    } else if duration_blocked {
        stats_events.push("studio_burn/burn/too_long".to_string());
    } else {
        if split_needed {
            stats_events.push("studio_burn/burn/split".to_string());
        }
        stats_events.push("studio_burn/burn/success".to_string());
    }

    (
        axum::http::StatusCode::OK,
        Json(StudioBurnResp {
            ok: true,
            sub_format: sub_format.to_string(),
            filter_type,
            filter_arg,
            order_used: order,
            route_decision,
            sub_workdir_name,
            video_workdir_name,
            sanitized_display_name,
            max_duration_secs,
            duration_blocked,
            source_codec,
            video_encoder,
            video_encoder_args,
            max_upload_bytes,
            split_needed,
            split_parts_planned,
            split_part_bytes_max,
            split_segment_secs,
            status_splitting_text,
            job_done_part_rendered_text,
            prompt_text,
            prompt_keyboard,
            job_cancel_keyboard,
            video_received_text,
            sub_received_text,
            sub_replaced_text,
            status_downloading_text,
            status_burning_text,
            status_uploading_text,
            job_done_rendered_text,
            job_cancelled_text,
            unsupported_sub_err_text,
            too_long_err_text,
            download_failed_err_text,
            oversized_err_text,
            burn_failed_err_text,
            stats_events,
            trace,
        }),
    )
}
