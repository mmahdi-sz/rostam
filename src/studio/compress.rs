//! Video Compression module for Photo & Video Magic Studio (`studio_compress`).
//!
//! Provides interactive UI for tuning codec (h264, h265, vp9, av1), resolution,
//! framerate (FPS), and bitrate ratio, with Redis session state storage and CPU Broker execution.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Instant;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{DeleteMessageParams, EditMessageTextParams, SendDocumentParams, SendMessageParams},
    types::{InlineKeyboardMarkup, Message, ReplyMarkup},
};
use redis::aio::MultiplexedConnection;
use serde::{Deserialize, Serialize};

use crate::bot::constants::{
    CB_START_STUDIO, CB_STUDIO_COMPRESS, CB_STUDIO_COMPRESS_CANCEL, CB_STUDIO_COMPRESS_JOBCANCEL,
    CB_STUDIO_COMPRESS_START,
};
use crate::emoji::panel::{btn_icon, btn_icon_danger, btn_icon_primary, btn_icon_success};
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::log::next_trace_id;
use crate::moebius::cpu::{acquire_cpu, pin_current_thread, release_cpu, trim_memory};
use crate::studio::pipeline::{TempDirGuard, cancel_active_job, register_active_job, remove_active_job};
use crate::studio::trim::run_ffprobe;

const SESSION_TTL_SECS: u64 = 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressSession {
    pub file_id: String,
    pub filename: String,
    pub orig_w: u32,
    pub orig_h: u32,
    pub orig_fps: u32,
    pub orig_bitrate: u64, // in bps from ffprobe
    pub orig_codec: String,
    pub orig_size_bytes: u64,
    pub duration_secs: u64,

    // Current user selections
    pub codec: String,  // "h264", "h265", "vp9", "av1"
    pub res_h: u32,     // 2160, 1440, 1080, 720, 480, 360, 240, 144
    pub fps: u32,       // 60, 45, 30, 24, 20, 15, 13
    pub br_ratio: u32,  // 100, 75, 50, 25, 16, 12
}

fn redis_key(user_id: i64) -> String {
    format!("studio_comp_session:{user_id}")
}

async fn redis_conn() -> redis::RedisResult<MultiplexedConnection> {
    let client = redis::Client::open(crate::config::redis_url())?;
    client.get_multiplexed_async_connection().await
}

pub async fn load_session(user_id: i64) -> Option<CompressSession> {
    let Ok(mut c) = redis_conn().await else {
        return None;
    };
    let val: Option<String> = redis::cmd("GET")
        .arg(redis_key(user_id))
        .query_async(&mut c)
        .await
        .ok()
        .flatten();
    val.as_deref().and_then(|s| serde_json::from_str(s).ok())
}

pub async fn save_session(user_id: i64, session: &CompressSession) {
    let Ok(mut c) = redis_conn().await else {
        return;
    };
    if let Ok(json) = serde_json::to_string(session) {
        let _: Result<(), _> = redis::cmd("SET")
            .arg(redis_key(user_id))
            .arg(json)
            .arg("EX")
            .arg(SESSION_TTL_SECS)
            .query_async::<()>(&mut c)
            .await;
    }
}

pub async fn clear_session(user_id: i64) {
    let Ok(mut c) = redis_conn().await else {
        return;
    };
    let _: Result<i64, _> = redis::cmd("DEL")
        .arg(redis_key(user_id))
        .query_async(&mut c)
        .await;
}

/// Computes bitrate in kbps for the given resolution height and ratio percentage.
pub fn calculate_target_bitrate_kbps(session: &CompressSession, target_h: u32, ratio_percent: u32) -> u64 {
    let orig_w = session.orig_w.max(1) as f64;
    let orig_h = session.orig_h.max(1) as f64;
    let target_w = (orig_w * target_h as f64 / orig_h).round();
    
    let orig_pixels = orig_w * orig_h;
    let target_pixels = target_w * target_h as f64;
    let pixel_ratio = (target_pixels / orig_pixels).min(1.0);

    let orig_kbps = (session.orig_bitrate as f64 / 1000.0).max(100.0);
    let base_target_kbps = orig_kbps * pixel_ratio;
    
    let final_kbps = base_target_kbps * (ratio_percent as f64 / 100.0);
    (final_kbps.round() as u64).max(50)
}

/// Computes estimated output file size in MB.
#[allow(dead_code)]
pub fn calculate_estimated_size_mb(session: &CompressSession, target_h: u32, ratio_percent: u32) -> f64 {
    let bitrate_kbps = calculate_target_bitrate_kbps(session, target_h, ratio_percent);
    let total_bits = (bitrate_kbps * 1000) as f64 * session.duration_secs as f64;
    (total_bits / 8.0) / (1024.0 * 1024.0)
}

/// Renders the inline keyboard for the compression menu.
pub fn build_compress_keyboard(session: &CompressSession) -> InlineKeyboardMarkup {
    let mut rows = Vec::new();

    // Section 1: Codec
    let codecs = [("h264", "H.264"), ("h265", "H.265"), ("vp9", "VP9"), ("av1", "AV1")];
    let mut codec_row = Vec::new();
    for (key, label) in codecs {
        let cb = format!("stc:set:c:{key}");
        let btn = if session.codec == key {
            btn_icon_success(label, &cb, "")
        } else {
            btn_icon(label, &cb, "")
        };
        codec_row.push(btn);
    }
    rows.push(codec_row);

    // Section 2: Resolution (Filtered by <= orig_h)
    let res_matrix: &[&[(u32, &str)]] = &[
        &[(2160, "2160p (4K)"), (1440, "1440p (2K)")],
        &[(1080, "1080p (fullHD)"), (720, "720p (HD)")],
        &[(480, "480p (SD)"), (360, "360p"), (240, "240p"), (144, "144p")],
    ];

    for row in res_matrix {
        let mut res_row = Vec::new();
        for &(h, label) in *row {
            if h <= session.orig_h {
                let cb = format!("stc:set:r:{h}");
                let btn = if session.res_h == h {
                    btn_icon_success(label, &cb, "")
                } else {
                    btn_icon(label, &cb, "")
                };
                res_row.push(btn);
            }
        }
        if !res_row.is_empty() {
            rows.push(res_row);
        }
    }

    // Section 3: FPS (Filtered by <= orig_fps)
    let fps_matrix: &[&[u32]] = &[
        &[60, 45, 30, 24],
        &[20, 15, 13],
    ];

    for row in fps_matrix {
        let mut fps_row = Vec::new();
        for &f in *row {
            if f <= session.orig_fps {
                let label = format!("{f} fps");
                let cb = format!("stc:set:f:{f}");
                let btn = if session.fps == f {
                    btn_icon_success(&label, &cb, "")
                } else {
                    btn_icon(&label, &cb, "")
                };
                fps_row.push(btn);
            }
        }
        if !fps_row.is_empty() {
            rows.push(fps_row);
        }
    }

    // Section 4: Bitrate Ratio (Calculated kbps)
    let br_matrix: &[&[u32]] = &[
        &[100, 75, 50],
        &[25, 16, 12],
    ];

    for row in br_matrix {
        let mut br_row = Vec::new();
        for &r in *row {
            let kbps = calculate_target_bitrate_kbps(session, session.res_h, r);
            let label = format!("{kbps} kbps");
            let cb = format!("stc:set:b:{r}");
            let btn = if session.br_ratio == r {
                btn_icon_success(&label, &cb, "")
            } else {
                btn_icon(&label, &cb, "")
            };
            br_row.push(btn);
        }
        if !br_row.is_empty() {
            rows.push(br_row);
        }
    }

    // Section 5: Actions
    rows.push(vec![btn_icon_success(
        &t("studio.compress.confirm_btn"),
        CB_STUDIO_COMPRESS_START,
        "rocket",
    )]);
    rows.push(vec![btn_icon_primary(
        &t("studio.back_to_studio"),
        CB_START_STUDIO,
        "back",
    )]);

    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

/// Renders the MarkdownV2 text for the compression menu.
pub fn build_compress_text(session: &CompressSession) -> String {
    let orig_res = format!("{}x{}", session.orig_w, session.orig_h);
    let orig_bitrate_kbps = session.orig_bitrate / 1000;
    let orig_size_mb = (session.orig_size_bytes as f64) / (1024.0 * 1024.0);
    let orig_size_str = format!("{orig_size_mb:.1}");
    
    let sel_codec = match session.codec.as_str() {
        "h264" => "H.264",
        "h265" => "H.265 (HEVC)",
        "vp9" => "VP9",
        "av1" => "AV1",
        _ => session.codec.as_str(),
    };
    let sel_res = format!("{}p", session.res_h);
    
    let sel_br_kbps = calculate_target_bitrate_kbps(session, session.res_h, session.br_ratio);
    let sel_br_label = format!("{sel_br_kbps} kbps");

    let container = if session.codec == "h264" { ".mp4" } else { ".mkv" };

    let raw = tf(
        "studio.compress.menu_title",
        &[
            ("orig_res", &md_escape(&orig_res)),
            ("orig_fps", &session.orig_fps.to_string()),
            ("orig_codec", &md_escape(&session.orig_codec)),
            ("orig_bitrate", &orig_bitrate_kbps.to_string()),
            ("orig_size", &md_escape(&orig_size_str)),
            ("sel_codec", &md_escape(sel_codec)),
            ("sel_res", &md_escape(&sel_res)),
            ("sel_fps", &session.fps.to_string()),
            ("sel_br_label", &md_escape(&sel_br_label)),
            ("container", &md_escape(container)),
        ],
    );

    apply_premium_to_md(&raw)
}

pub fn format_eta_hms(secs: u64) -> String {
    let hours = secs / 3600;
    let rem = secs % 3600;
    let mins = rem / 60;
    let seconds = rem % 60;

    let mut parts = Vec::new();
    if hours > 0 {
        parts.push(tf("studio.compress.eta_unit_hours", &[("n", &hours.to_string())]));
    }
    if mins > 0 {
        parts.push(tf("studio.compress.eta_unit_minutes", &[("n", &mins.to_string())]));
    }
    if seconds > 0 || parts.is_empty() {
        parts.push(tf("studio.compress.eta_unit_seconds", &[("n", &seconds.to_string())]));
    }
    parts.join(&t("studio.compress.eta_join_and"))
}

pub fn compute_vmaf_score(
    output_file: &std::path::Path,
    input_file: &std::path::Path,
    orig_w: u32,
    orig_h: u32,
    threads_arg: &str,
) -> String {
    let vmaf_filter = format!("[0:v]scale={orig_w}:{orig_h}[dist];[1:v][dist]libvmaf=n_threads={threads_arg}");
    let mut cmd = std::process::Command::new(crate::config::ffmpeg_path());
    cmd.args([
        "-i",
        output_file.to_str().unwrap_or_default(),
        "-i",
        input_file.to_str().unwrap_or_default(),
        "-filter_complex",
        &vmaf_filter,
        "-f",
        "null",
        "-",
    ])
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::piped());

    let Ok(out) = cmd.output() else {
        return "N/A".to_string();
    };

    let stderr = String::from_utf8_lossy(&out.stderr);
    for line in stderr.lines() {
        if let Some(pos) = line.find("VMAF score:") {
            let rest = &line[pos + "VMAF score:".len()..];
            if let Ok(score) = rest.trim().parse::<f64>() {
                return format!("{score:.2}");
            }
        }
    }
    "N/A".to_string()
}

pub async fn enter_compress_prompt(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.set(user_id, FlowState::AwaitingStudioCompressVideo);
    log_actor_id!("studio_compress", trace_id, user_id, "clicked" => CB_STUDIO_COMPRESS);

    let text = apply_premium_to_md(&t("studio.compress.send_video_prompt"));
    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("studio.compress.cancel_btn"),
            CB_STUDIO_COMPRESS_CANCEL,
            "cancel",
        )]])
        .build();

    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(kb)
        .build();

    let _ = api.edit_message_text(&params).await;
}

pub async fn handle_video_upload(
    api: &Bot,
    msg: Message,
    user_id: i64,
    trace_id: u64,
    flow_manager: &FlowManager,
) {
    let chat_id = msg.chat.id;

    if !super::is_video_message_metadata(&msg) {
        log_ev!("studio_compress", trace_id, "not_a_video_metadata", "=>" => "fail");
        let _ = crate::bot::send_text_md(api, chat_id, &t("studio.compress.error.not_a_video")).await;
        return;
    }

    let (file_id, raw_filename, orig_size_bytes) = if let Some(v) = msg.video.as_ref() {
        (
            v.file_id.clone(),
            v.file_name.as_deref().unwrap_or("video.mp4").to_string(),
            v.file_size.unwrap_or(0),
        )
    } else if let Some(d) = msg.document.as_ref() {
        (
            d.file_id.clone(),
            d.file_name.as_deref().unwrap_or("video.mp4").to_string(),
            d.file_size.unwrap_or(0),
        )
    } else {
        log_ev!("studio_compress", trace_id, "invalid_video_msg", "=>" => "fail");
        let _ = crate::bot::send_text_md(api, chat_id, &t("studio.compress.error.not_a_video")).await;
        return;
    };

    log_actor_id!("studio_compress", trace_id, user_id, "uploaded" => "video");
    let file_id_prefix = if file_id.len() >= 6 { &file_id[..6] } else { &file_id };
    log_ev!("studio_compress", trace_id, "video_received", "user_id" => user_id, "file_id" => file_id_prefix);

    let status_raw = tf(
        "studio.compress.status_downloading",
        &[("elapsed", &md_escape("0s")), ("detail", "")],
    );
    let status_text = apply_premium_to_md(&status_raw);
    let status_msg = match api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&status_text)
                .parse_mode(ParseMode::MarkdownV2)
                .build(),
        )
        .await
    {
        Ok(m) => m,
        Err(e) => {
            log_ev!("studio_compress", trace_id, "status_send_failed", "=>" => format!("fail err={e}"));
            return;
        }
    };

    let work_dir = std::env::temp_dir().join(format!("studio_compress_{trace_id}_{user_id}"));
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        log_ev!("studio_compress", trace_id, "mkdir_failed", "=>" => format!("fail err={e}"));
        let _ = crate::bot::send_text_md(api, chat_id, &t("studio.compress.error.download_failed")).await;
        return;
    }
    let _guard = TempDirGuard::new(work_dir.clone());

    let local_file = work_dir.join(&raw_filename);

    let dl_stop_flag = super::pipeline::spawn_download_ticker(
        api.clone(),
        chat_id,
        status_msg.result.message_id,
        local_file.clone(),
        orig_size_bytes,
        "studio.compress",
        None,
    );

    let dl_res = crate::bot::files::download_telegram_file(api, &file_id, &local_file).await;
    dl_stop_flag.store(true, Ordering::Relaxed);

    if let Err(e) = dl_res {
        log_ev!("studio_compress", trace_id, "download_failed", "=>" => format!("fail err={e}"));
        let _ = crate::bot::send_text_md(api, chat_id, &t("studio.compress.error.download_failed")).await;
        return;
    }


    let meta = match run_ffprobe(&local_file) {
        Ok(m) => m,
        Err(e) => {
            log_ev!("studio_compress", trace_id, "ffprobe_failed", "=>" => format!("fail err={e}"));
            let _ = crate::bot::send_text_md(api, chat_id, &t("studio.compress.error.ffprobe_failed")).await;
            return;
        }
    };

    // Determine initial defaults
    let orig_w = meta.width.max(1);
    let orig_h = meta.height.max(1);
    let orig_fps = meta.fps.max(1);
    let orig_bitrate = meta.bitrate.max(100_000);
    let duration_secs = meta.duration_secs.max(1);

    // Initial selected resolution height is orig_h or nearest lower standard height
    let initial_res_h = [2160, 1440, 1080, 720, 480, 360, 240, 144]
        .into_iter()
        .find(|&h| h <= orig_h)
        .unwrap_or(orig_h);

    // Initial selected FPS is orig_fps or nearest lower standard FPS
    let initial_fps = [60, 45, 30, 24, 20, 15, 13]
        .into_iter()
        .find(|&f| f <= orig_fps)
        .unwrap_or(orig_fps);

    let initial_codec = match meta.codec.to_lowercase().as_str() {
        "av1" | "svtav1" | "libsvtav1" => "av1",
        "h265" | "hevc" | "libx265" => "h265",
        "vp9" | "libvpx-vp9" => "vp9",
        _ => "h264",
    }
    .to_string();

    let session = CompressSession {
        file_id: file_id.clone(),
        filename: raw_filename.to_string(),
        orig_w,
        orig_h,
        orig_fps,
        orig_bitrate,
        orig_codec: meta.codec.clone(),
        orig_size_bytes,
        duration_secs,

        codec: initial_codec,
        res_h: initial_res_h,
        fps: initial_fps,
        br_ratio: 100,
    };

    save_session(user_id, &session).await;
    flow_manager.clear(user_id);

    let menu_text = build_compress_text(&session);
    let menu_kb = build_compress_keyboard(&session);

    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(status_msg.result.message_id)
        .text(&menu_text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(menu_kb)
        .build();

    let _ = api.edit_message_text(&params).await;
}

pub async fn handle_compress_cb(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    cb_data: &str,
    flow_manager: &FlowManager,
) -> bool {
    let trace_id = next_trace_id();
    log_ev!("studio_compress", trace_id, "callback", "cb" => cb_data, "user_id" => user_id);

    if cb_data == CB_STUDIO_COMPRESS_CANCEL {
        clear_session(user_id).await;
        flow_manager.clear(user_id);
        crate::studio::enter_studio(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data == CB_STUDIO_COMPRESS_JOBCANCEL {
        let cancelled = cancel_active_job(user_id);
        log_ev!("studio_compress", trace_id, "job_cancel_result", "cancelled" => cancelled);
        true
    } else if cb_data == CB_STUDIO_COMPRESS_START {
        let Some(session) = load_session(user_id).await else {
            let _ = crate::bot::send_text_md(api, chat_id, &t("studio.compress.error.compress_failed")).await;
            return true;
        };
        start_compression_job(api, chat_id, message_id, user_id, session, flow_manager).await;
        true
    } else if let Some(rest) = cb_data.strip_prefix("stc:set:") {
        let Some(mut session) = load_session(user_id).await else {
            return true;
        };
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() == 2 {
            match parts[0] {
                "c" => session.codec = parts[1].to_string(),
                "r" => {
                    if let Ok(h) = parts[1].parse::<u32>() {
                        if h <= session.orig_h {
                            session.res_h = h;
                        }
                    }
                }
                "f" => {
                    if let Ok(f) = parts[1].parse::<u32>() {
                        if f <= session.orig_fps {
                            session.fps = f;
                        }
                    }
                }
                "b" => {
                    if let Ok(b) = parts[1].parse::<u32>() {
                        if [100, 75, 50, 25, 16, 12].contains(&b) {
                            session.br_ratio = b;
                        }
                    }
                }
                _ => {}
            }
            save_session(user_id, &session).await;
            let menu_text = build_compress_text(&session);
            let menu_kb = build_compress_keyboard(&session);

            let params = EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(message_id)
                .text(&menu_text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(menu_kb)
                .build();

            let _ = api.edit_message_text(&params).await;
        }
        true
    } else {
        false
    }
}

pub async fn start_compression_job(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    session: CompressSession,
    flow_manager: &FlowManager,
) {
    if crate::moebius::cpu::is_user_cpu_busy(user_id).await {
        let _ = crate::bot::send_text_md(api, chat_id, &t("active_job_running")).await;
        return;
    }

    let trace_id = next_trace_id();
    log_actor_id!("studio_compress", trace_id, user_id, "start_job" => &session.codec);

    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_active_job(user_id, cancel_flag.clone());

    let flow_manager = flow_manager.clone();
    let api = api.clone();

    crate::app::spawn_user_task(async move {
        let cancel_kb = InlineKeyboardMarkup::builder()
            .inline_keyboard(vec![vec![btn_icon_danger(
                &t("studio.compress.cancel_btn"),
                CB_STUDIO_COMPRESS_JOBCANCEL,
                "cancel",
            )]])
            .build();

        let status_text = apply_premium_to_md(&t("studio.compress.status_downloading"));
        let params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(&status_text)
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(cancel_kb.clone())
            .build();

        let _ = api.edit_message_text(&params).await;

        if cancel_flag.load(Ordering::Relaxed) {
            remove_active_job(user_id);
            clear_session(user_id).await;
            let _ = crate::bot::send_text_md(&api, chat_id, &t("studio.compress.job_cancelled")).await;
            crate::studio::send_studio_menu_new_msg(&api, chat_id, user_id, &flow_manager).await;
            return;
        }

        let work_dir = std::env::temp_dir().join(format!("studio_comp_run_{trace_id}_{user_id}"));
        if let Err(e) = std::fs::create_dir_all(&work_dir) {
            log_ev!("studio_compress", trace_id, "mkdir_failed", "=>" => format!("fail err={e}"));
            remove_active_job(user_id);
            let _ = crate::bot::send_text_md(&api, chat_id, &t("studio.compress.error.download_failed")).await;
            return;
        }
        let _guard = TempDirGuard::new(work_dir.clone());

        let input_file = work_dir.join(&session.filename);
        let download_start = Instant::now();
        let stats_job_id = crate::stats::record_download_start(user_id, "studio_compress").await;

        let dl_result = match crate::bot::files::download_telegram_file(&api, &session.file_id, &input_file).await {
            Ok(res) => res,
            Err(e) => {
                log_ev!("studio_compress", trace_id, "download_failed", "=>" => format!("fail err={e}"));
                remove_active_job(user_id);
                let _ = crate::bot::send_text_md(&api, chat_id, &t("studio.compress.error.download_failed")).await;
                return;
            }
        };

        if let Some(jid) = stats_job_id {
            crate::stats::record_download_done(
                jid,
                dl_result.bytes as i64,
                None,
                None,
                Some(dl_result.speed_bps() as i64),
            )
            .await;
        }
        let download_secs = download_start.elapsed().as_secs();


        if cancel_flag.load(Ordering::Relaxed) {
            remove_active_job(user_id);
            clear_session(user_id).await;
            let _ = crate::bot::send_text_md(&api, chat_id, &t("studio.compress.job_cancelled")).await;
            crate::studio::send_studio_menu_new_msg(&api, chat_id, user_id, &flow_manager).await;
            return;
        }

        // Acquire CPU broker
        let cores = acquire_cpu(user_id, trace_id).await;
        let threads_arg = if !cores.is_empty() {
            cores.len().to_string()
        } else {
            "2".to_string()
        };

        if cancel_flag.load(Ordering::Relaxed) {
            release_cpu(cores, trace_id).await;
            remove_active_job(user_id);
            clear_session(user_id).await;
            let _ = crate::bot::send_text_md(&api, chat_id, &t("studio.compress.job_cancelled")).await;
            crate::studio::send_studio_menu_new_msg(&api, chat_id, user_id, &flow_manager).await;
            return;
        }

        // Output format & container
        let ext = if session.codec == "h264" { "mp4" } else { "mkv" };
        let codec_tag = session.codec.to_uppercase();
        let file_stem = std::path::Path::new(&session.filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("video");
        let output_file = work_dir.join(format!("{file_stem}_{codec_tag}.{ext}"));

        // FFmpeg video encoder mapping
        let vcodec_flag = match session.codec.as_str() {
            "h264" => "libx264",
            "h265" => "libx265",
            "vp9" => "libvpx-vp9",
            "av1" => "libsvtav1",
            _ => "libx264",
        };

        let target_kbps = calculate_target_bitrate_kbps(&session, session.res_h, session.br_ratio);
        let scale_filter = format!("scale=-2:{}", session.res_h);
        let r_flag = session.fps.to_string();
        let b_v_flag = format!("{target_kbps}k");

        // Live ticker with ETA calculation
        let job_start = Instant::now();
        let stop_ticker = Arc::new(AtomicBool::new(false));
        let progress_pct = Arc::new(AtomicU8::new(0));
        {
            let stop_ticker_inner = stop_ticker.clone();
            let progress_pct_inner = progress_pct.clone();
            let api_inner = api.clone();
            crate::app::spawn_user_task(async move {
                let mut last_rendered = String::new();
                while !stop_ticker_inner.load(Ordering::Relaxed) {
                    let elapsed_secs = job_start.elapsed().as_secs();
                    let elapsed_str = format_eta_hms(elapsed_secs);
                    let pct = progress_pct_inner.load(Ordering::Relaxed);

                    let eta_param = if pct > 0 && pct < 100 {
                        let total_est = elapsed_secs as f64 * 100.0 / pct as f64;
                        let rem_secs = (total_est - elapsed_secs as f64).max(0.0) as u64;
                        let eta_str = format_eta_hms(rem_secs);
                        tf(
                            "studio.compress.status_job_ticker_eta",
                            &[("eta", &md_escape(&eta_str))],
                        )
                    } else {
                        String::new()
                    };

                    let render_key = format!("{elapsed_secs}:{pct}");
                    if render_key != last_rendered {
                        last_rendered = render_key;
                        let ticker_raw = tf(
                            "studio.compress.status_job_ticker",
                            &[("elapsed", &md_escape(&elapsed_str)), ("eta", &eta_param)],
                        );
                        let text = apply_premium_to_md(&ticker_raw);
                        let edit_params = EditMessageTextParams::builder()
                            .chat_id(chat_id)
                            .message_id(message_id)
                            .text(&text)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(cancel_kb.clone())
                            .build();
                        let _ = api_inner.edit_message_text(&edit_params).await;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                }
            });
        }

        let preset_flag = if session.codec == "av1" { "9" } else { "medium" };
        let input_path = input_file.clone();
        let output_path = output_file.clone();
        let vcodec_str = vcodec_flag.to_string();
        let cores_clone = cores.clone();
        let cancel_flag_inner = cancel_flag.clone();
        let progress_pct_inner = progress_pct.clone();
        let duration_secs = session.duration_secs;
        let threads_arg_inner = threads_arg.clone();

        let run_res = tokio::task::spawn_blocking(move || {
            pin_current_thread(&cores_clone, trace_id);

            let mut cmd = std::process::Command::new(crate::config::ffmpeg_path());
            cmd.args([
                "-y",
                "-progress",
                "pipe:1",
                "-i",
                input_path.to_str().unwrap_or_default(),
                "-c:v",
                &vcodec_str,
                "-preset",
                preset_flag,
                "-b:v",
                &b_v_flag,
                "-r",
                &r_flag,
                "-vf",
                &scale_filter,
                "-threads",
                &threads_arg_inner,
                "-c:a",
                "copy",
            ]);
            if output_path.extension().and_then(|e| e.to_str()).unwrap_or("") == "mp4" {
                cmd.args(["-movflags", "+faststart"]);
            }
            cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .arg(&output_path);

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => return Err(anyhow::anyhow!("ffmpeg spawn error: {e}")),
            };

            if let Some(stdout_stream) = child.stdout.take() {
                let pct_flag = progress_pct_inner.clone();
                std::thread::spawn(move || {
                    use std::io::{BufRead, BufReader};
                    let reader = BufReader::new(stdout_stream);
                    for line in reader.lines().map_while(Result::ok) {
                        if let Some(us_str) = line.strip_prefix("out_time_us=") {
                            if let Ok(us) = us_str.trim().parse::<u64>() {
                                let proc_secs = us / 1_000_000;
                                if duration_secs > 0 {
                                    let pct = ((proc_secs as f64 / duration_secs as f64) * 100.0)
                                        .clamp(0.0, 99.0) as u8;
                                    pct_flag.store(pct, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                });
            }

            let mut success = false;
            while !cancel_flag_inner.load(Ordering::Relaxed) {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        success = status.success()
                            && output_path.exists()
                            && std::fs::metadata(&output_path).map(|m| m.len() > 0).unwrap_or(false);
                        break;
                    }
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
                    Err(_) => break,
                }
            }

            if cancel_flag_inner.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                trim_memory();
                return Ok(false);
            }

            trim_memory();
            Ok(success)
        })
        .await;

        stop_ticker.store(true, Ordering::Relaxed);
        release_cpu(cores, trace_id).await;

        let compress_secs = job_start.elapsed().as_secs();

        if cancel_flag.load(Ordering::Relaxed) {
            remove_active_job(user_id);
            clear_session(user_id).await;
            let _ = crate::bot::send_text_md(&api, chat_id, &t("studio.compress.job_cancelled")).await;
            crate::studio::send_studio_menu_new_msg(&api, chat_id, user_id, &flow_manager).await;
            return;
        }

        let ffmpeg_ok = match run_res {
            Ok(Ok(true)) => true,
            _ => false,
        };

        if !ffmpeg_ok || !output_file.exists() {
            log_ev!("studio_compress", trace_id, "ffmpeg_failed", "=>" => "fail");
            remove_active_job(user_id);
            let _ = crate::bot::send_text_md(&api, chat_id, &t("studio.compress.error.compress_failed")).await;
            return;
        }

        // Upload output document (file) with completion caption
        let output_len = std::fs::metadata(&output_file).map(|m| m.len()).unwrap_or(0);
        let final_size_mb = (output_len as f64) / (1024.0 * 1024.0);
        let final_size_str = format!("{final_size_mb:.1}");
        let orig_size_mb = (session.orig_size_bytes as f64) / (1024.0 * 1024.0);
        let orig_size_str = format!("{orig_size_mb:.1}");
        
        let saved_percent = if session.orig_size_bytes > 0 && output_len < session.orig_size_bytes {
            (((session.orig_size_bytes as f64 - output_len as f64) / session.orig_size_bytes as f64) * 100.0).round() as u32
        } else {
            0
        };

        let upload_secs = job_start.elapsed().as_secs().saturating_sub(download_secs + compress_secs);
        let vmaf_score = compute_vmaf_score(&output_file, &input_file, session.orig_w, session.orig_h, &threads_arg);

        let done_raw = tf(
            "studio.compress.job_done",
            &[
                ("orig_size", &md_escape(&orig_size_str)),
                ("final_size", &md_escape(&final_size_str)),
                ("saved_percent", &saved_percent.to_string()),
                ("compress_time", &md_escape(&format_eta_hms(compress_secs))),
                ("download_time", &md_escape(&format_eta_hms(download_secs))),
                ("upload_time", &md_escape(&format_eta_hms(upload_secs))),
                ("vmaf_score", &md_escape(&vmaf_score)),
            ],
        );
        let done_text = apply_premium_to_md(&done_raw);

        let send_params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(FileUpload::InputFile(InputFile {
                path: output_file.clone(),
            }))
            .caption(&done_text)
            .parse_mode(ParseMode::MarkdownV2)
            .build();

        let out_bytes = std::fs::metadata(&output_file).map(|m| m.len()).unwrap_or(0);
        let up_start = std::time::Instant::now();

        use crate::bot::send_file_with_upload_ticker;
        let send_res = send_file_with_upload_ticker::<_, frankenstein::types::Message>(
            &api,
            "sendDocument",
            &send_params,
            &output_file,
            chat_id,
            message_id,
            "transfer.stage.sending_document",
            None,
        ).await;
        remove_active_job(user_id);
        clear_session(user_id).await;

        if let Err(e) = send_res {
            log_ev!("studio_compress", trace_id, "upload_failed", "=>" => format!("fail err={e}"));
            let _ = crate::bot::send_text_md(&api, chat_id, &t("studio.compress.error.compress_failed")).await;
            return;
        }

        let up_elapsed = up_start.elapsed();
        let up_speed = if up_elapsed.as_secs_f64() > 0.0 {
            out_bytes as f64 / up_elapsed.as_secs_f64()
        } else {
            0.0
        };
        if let Some(jid) = stats_job_id {
            crate::stats::record_upload_done(
                jid,
                user_id,
                out_bytes as i64,
                Some(up_speed as i64),
                Some(1),
            )
            .await;
        }


        // Delete status message
        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(message_id)
                    .build(),
            )
            .await;

        // Re-arm flow with a NEW prompt message
        send_compress_prompt_new_msg(&api, chat_id, user_id, &flow_manager).await;
    });
}

pub async fn send_compress_prompt_new_msg(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.set(user_id, FlowState::AwaitingStudioCompressVideo);
    log_actor_id!("studio_compress", trace_id, user_id, "rearm" => "prompt");

    let text = apply_premium_to_md(&t("studio.compress.send_video_prompt"));
    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("studio.compress.cancel_btn"),
            CB_STUDIO_COMPRESS_CANCEL,
            "cancel",
        )]])
        .build();

    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(kb))
        .build();

    let _ = api.send_message(&params).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_target_bitrate_and_estimated_size() {
        let session = CompressSession {
            file_id: "fid".into(),
            filename: "v.mp4".into(),
            orig_w: 1920,
            orig_h: 1080,
            orig_fps: 30,
            orig_bitrate: 2_000_000,
            orig_codec: "h264".into(),
            orig_size_bytes: 30_000_000,
            duration_secs: 120,
            codec: "h264".into(),
            res_h: 720,
            fps: 30,
            br_ratio: 75,
        };

        let target_kbps = calculate_target_bitrate_kbps(&session, 720, 75);
        assert!(target_kbps > 0);
        let est_mb = calculate_estimated_size_mb(&session, 720, 75);
        assert!(est_mb > 0.0);
    }

    #[test]
    fn test_format_eta_hms() {
        assert_eq!(format_eta_hms(45), "45 ثانیه");
        assert_eq!(format_eta_hms(970), "16 دقیقه و 10 ثانیه");
        assert_eq!(format_eta_hms(3912), "1 ساعت و 5 دقیقه و 12 ثانیه");
    }

    #[test]
    fn test_status_downloading_no_raw_braces() {
        // Reproduces the MarkdownV2 parse error: "Character '{' is reserved"
        // that fires in handle_video_upload when the first send_message call fails.
        let status_raw = crate::i18n::tf(
            "studio.compress.status_downloading",
            &[("elapsed", &crate::i18n::md_escape("0s")), ("detail", "")],
        );
        let status_text = crate::i18n::apply_premium_to_md(&status_raw);
        println!("status_text = {:?}", status_text);
        assert!(
            !status_text.contains('{') && !status_text.contains('}'),
            "MarkdownV2 status text still has raw braces: {:?}",
            status_text
        );

        // Also test via start_compression_job path: t() without tf() leaves placeholders
        let bad_text = crate::i18n::apply_premium_to_md(&crate::i18n::t("studio.compress.status_downloading"));
        println!("bad_text (t without tf) = {:?}", bad_text);
        let has_braces = bad_text.contains('{') || bad_text.contains('}');
        println!("Has unescaped braces (start_compression_job bug): {}", has_braces);
    }
}
