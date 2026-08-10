use axum::Json;
use serde::{Deserialize, Serialize};

use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::studio::trim::{
    DEFAULT_MAX_CUT_RANGES, RangeError, cancel_keyboard,
    format_timestamp, parse_cut_ranges,
};

#[derive(Deserialize)]
pub struct StudioTrimReq {
    #[allow(dead_code)]
    pub user_id: Option<i64>,
    pub input_ranges: Option<String>,
    pub duration_secs: Option<u64>,
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
    pub errors: Vec<String>,
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
    let input = req.input_ranges.unwrap_or_else(|| "00:00 - 00:30\n۰۰:۰۱:۰۰ - ۰۰:۰۲:۰۰".to_string());
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
                            &[
                                ("line", &line_idx.to_string()),
                                ("text", &md_escape(&text)),
                            ],
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

    let raw_ticker = tf(
        "studio.trim.status_job_ticker",
        &[
            ("current", "1"),
            ("total", "2"),
            ("elapsed", &md_escape("5s")),
        ],
    );
    let status_job_ticker = apply_premium_to_md(&raw_ticker);

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
            errors: err_strings,
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
}

pub async fn test_studio_compress(
    Json(req): Json<StudioCompressReq>,
) -> (axum::http::StatusCode, Json<StudioCompressResp>) {
    use crate::studio::compress::{
        CompressSession, build_compress_keyboard, build_compress_text,
        calculate_estimated_size_mb, calculate_target_bitrate_kbps, format_eta_hms,
    };

    let orig_w = req.orig_w.unwrap_or(1920);
    let orig_h = req.orig_h.unwrap_or(1080);
    let orig_fps = req.orig_fps.unwrap_or(60);
    let orig_bitrate = req.orig_bitrate.unwrap_or(2_000_000);
    let duration_secs = req.duration_secs.unwrap_or(120);

    let res_h = req.selected_res.unwrap_or(1080).min(orig_h);
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
    let available_resolutions: Vec<u32> = all_res.into_iter().filter(|&h| h <= orig_h).collect();

    let all_fps = [60, 45, 30, 24, 20, 15, 13];
    let available_fps: Vec<u32> = all_fps.into_iter().filter(|&f| f <= orig_fps).collect();

    let calculated_target_bitrate_kbps = calculate_target_bitrate_kbps(&session, res_h, br_ratio);
    let estimated_size_mb = calculate_estimated_size_mb(&session, res_h, br_ratio);
    let container = if codec == "h264" { ".mp4".to_string() } else { ".mkv".to_string() };

    let preset = if codec == "av1" { "9".to_string() } else { "medium".to_string() };

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

    let job_done_sample = apply_premium_to_md(&tf(
        "studio.compress.job_done",
        &[
            ("orig_size", &md_escape("903.4")),
            ("final_size", &md_escape("597.2")),
            ("saved_percent", "34"),
            ("compress_time", &md_escape(&format_eta_hms(1293))),
            ("download_time", &md_escape(&format_eta_hms(0))),
            ("upload_time", &md_escape(&format_eta_hms(0))),
            ("vmaf_score", &md_escape("94.23")),
        ],
    ));

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
        }),
    )
}
