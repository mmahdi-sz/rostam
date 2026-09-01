use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Instant;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{DeleteMessageParams, EditMessageTextParams, SendDocumentParams},
    types::InlineKeyboardMarkup,
};

use super::calc::{
    calculate_target_bitrate_kbps, calculate_target_dimensions, compute_vmaf_score, format_eta_hms,
};
use super::session::{CompressSession, clear_session};
use super::ui::send_compress_prompt_new_msg;
use crate::bot::constants::CB_STUDIO_COMPRESS_JOBCANCEL;
use crate::common::cpu_broker::CpuBrokerGuard;
use crate::emoji::FlowManager;
use crate::emoji::panel::btn_icon_danger;
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::log::next_trace_id;
use crate::moebius::cpu::trim_memory;
use crate::studio::pipeline::{TempDirGuard, job_guard, register_active_job};

pub async fn start_compression_job(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    session: CompressSession,
    flow_manager: &FlowManager,
) {
    if CpuBrokerGuard::is_user_busy(user_id).await {
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
        let _job_guard = job_guard(user_id);
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
            clear_session(user_id).await;
            let _ =
                crate::bot::send_text_md(&api, chat_id, &t("studio.compress.job_cancelled")).await;
            crate::studio::send_studio_menu_new_msg(&api, chat_id, user_id, &flow_manager).await;
            return;
        }

        let work_dir = std::env::temp_dir().join(format!("studio_comp_run_{trace_id}_{user_id}"));
        if let Err(e) = std::fs::create_dir_all(&work_dir) {
            log_ev!("studio_compress", trace_id, "mkdir_failed", "=>" => format!("fail err={e}"));

            let _ = crate::bot::send_text_md(
                &api,
                chat_id,
                &t("studio.compress.error.download_failed"),
            )
            .await;
            return;
        }
        let _guard = TempDirGuard::new(work_dir.clone());

        let input_file = work_dir.join(&session.filename);
        let download_start = Instant::now();
        let stats_job_id = crate::stats::record_download_start(user_id, "studio_compress").await;

        let dl_result = match crate::bot::files::download_telegram_file(
            &api,
            &session.file_id,
            &input_file,
        )
        .await
        {
            Ok(res) => res,
            Err(e) => {
                log_ev!("studio_compress", trace_id, "download_failed", "=>" => format!("fail err={e}"));

                let _ = crate::bot::send_text_md(
                    &api,
                    chat_id,
                    &t("studio.compress.error.download_failed"),
                )
                .await;
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
            clear_session(user_id).await;
            let _ =
                crate::bot::send_text_md(&api, chat_id, &t("studio.compress.job_cancelled")).await;
            crate::studio::send_studio_menu_new_msg(&api, chat_id, user_id, &flow_manager).await;
            return;
        }

        // Acquire CPU broker
        let mut cpu_guard = CpuBrokerGuard::acquire(user_id, trace_id, "studio_compress").await;
        let threads_arg = if !cpu_guard.cores().is_empty() {
            cpu_guard.cores().len().to_string()
        } else {
            "2".to_string()
        };

        if cancel_flag.load(Ordering::Relaxed) {
            cpu_guard.release().await;

            clear_session(user_id).await;
            let _ =
                crate::bot::send_text_md(&api, chat_id, &t("studio.compress.job_cancelled")).await;
            crate::studio::send_studio_menu_new_msg(&api, chat_id, user_id, &flow_manager).await;
            return;
        }

        // Output format & container
        let ext = if session.codec == "h264" {
            "mp4"
        } else {
            "mkv"
        };
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
        let (target_w, target_h) =
            calculate_target_dimensions(session.orig_w, session.orig_h, session.res_h);
        let scale_filter = if target_w == session.orig_w && target_h == session.orig_h {
            "scale=trunc(iw/2)*2:trunc(ih/2)*2".to_string()
        } else if session.orig_w >= session.orig_h {
            format!("scale=-2:{}", session.res_h)
        } else {
            format!("scale={}:-2", session.res_h)
        };
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

        let preset_flag = if session.codec == "av1" {
            "9"
        } else {
            "medium"
        };
        let input_path = input_file.clone();
        let output_path = output_file.clone();
        let vcodec_str = vcodec_flag.to_string();
        let cores_clone = cpu_guard.cores().to_vec();
        let cancel_flag_inner = cancel_flag.clone();
        let progress_pct_inner = progress_pct.clone();
        let duration_secs = session.duration_secs;
        let threads_arg_inner = threads_arg.clone();

        let run_res = tokio::task::spawn_blocking(move || {
            if !cores_clone.is_empty() {
                crate::moebius::cpu::pin_current_thread(&cores_clone, trace_id);
            }

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
            if output_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                == "mp4"
            {
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
                                        .clamp(0.0, 99.0)
                                        as u8;
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
                            && std::fs::metadata(&output_path)
                                .map(|m| m.len() > 0)
                                .unwrap_or(false);
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
        cpu_guard.release().await;

        let compress_secs = job_start.elapsed().as_secs();

        if cancel_flag.load(Ordering::Relaxed) {
            clear_session(user_id).await;
            let _ =
                crate::bot::send_text_md(&api, chat_id, &t("studio.compress.job_cancelled")).await;
            crate::studio::send_studio_menu_new_msg(&api, chat_id, user_id, &flow_manager).await;
            return;
        }

        let ffmpeg_ok = matches!(run_res, Ok(Ok(true)));

        if !ffmpeg_ok || !output_file.exists() {
            log_ev!("studio_compress", trace_id, "ffmpeg_failed", "=>" => "fail");

            let _ = crate::bot::send_text_md(
                &api,
                chat_id,
                &t("studio.compress.error.compress_failed"),
            )
            .await;
            return;
        }

        // Upload output document (file) with completion caption
        let output_len = std::fs::metadata(&output_file)
            .map(|m| m.len())
            .unwrap_or(0);
        let final_size_mb = (output_len as f64) / (1024.0 * 1024.0);
        let final_size_str = format!("{final_size_mb:.1}");
        let orig_size_mb = (session.orig_size_bytes as f64) / (1024.0 * 1024.0);
        let orig_size_str = format!("{orig_size_mb:.1}");

        let saved_percent = if session.orig_size_bytes > 0 && output_len < session.orig_size_bytes {
            (((session.orig_size_bytes as f64 - output_len as f64)
                / session.orig_size_bytes as f64)
                * 100.0)
                .round() as u32
        } else {
            0
        };

        let upload_secs = job_start
            .elapsed()
            .as_secs()
            .saturating_sub(download_secs + compress_secs);
        let vmaf_score = compute_vmaf_score(
            &output_file,
            &input_file,
            session.orig_w,
            session.orig_h,
            &threads_arg,
        );

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

        let out_bytes = std::fs::metadata(&output_file)
            .map(|m| m.len())
            .unwrap_or(0);
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
        )
        .await;
        clear_session(user_id).await;

        if let Err(e) = send_res {
            log_ev!("studio_compress", trace_id, "upload_failed", "=>" => format!("fail err={e}"));
            let _ = crate::bot::send_text_md(
                &api,
                chat_id,
                &t("studio.compress.error.compress_failed"),
            )
            .await;
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
