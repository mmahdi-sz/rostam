use std::path::PathBuf;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::SendAudioParams,
};

use crate::bot::send_file_with_upload_ticker;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{entities_for_text, t, tf};

use super::format::format_clock;
use super::keyboards::prompt_keyboard;
use super::log_trace;
use super::types::SeparationResult;

pub async fn deliver_separation_results(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    tmp_dir: &std::path::Path,
    result: SeparationResult,
    stats_job_id: Option<i64>,
    audio_duration_secs: u64,
    mode_label: &str,
    op_started: std::time::Instant,
    flow_manager: &FlowManager,
    trace_id: u64,
) {
    log_trace(
        trace_id,
        "separate_done",
        &format!(
            "duration={:.1}s vocals_wav={} instrumental_wav={} vocals_compressed={} instrumental_compressed={} ext={}",
            result.duration_seconds,
            result.vocals_wav.len(),
            result.instrumental_wav.len(),
            result.vocals_compressed.len(),
            result.instrumental_compressed.len(),
            result.compressed_ext
        ),
    );

    let vocals_wav_path = tmp_dir.join("vocals.wav");
    let instrumental_wav_path = tmp_dir.join("instrumental.wav");
    let vocals_compressed_path = tmp_dir.join(format!("vocals.{}", result.compressed_ext));
    let instrumental_compressed_path =
        tmp_dir.join(format!("instrumental.{}", result.compressed_ext));

    std::fs::write(&vocals_wav_path, &result.vocals_wav).ok();
    std::fs::write(&instrumental_wav_path, &result.instrumental_wav).ok();
    std::fs::write(&vocals_compressed_path, &result.vocals_compressed).ok();
    std::fs::write(
        &instrumental_compressed_path,
        &result.instrumental_compressed,
    )
    .ok();

    let up_start = std::time::Instant::now();
    let total_out_bytes = (std::fs::metadata(&vocals_compressed_path)
        .map(|m| m.len())
        .unwrap_or(0))
        + (std::fs::metadata(&vocals_wav_path)
            .map(|m| m.len())
            .unwrap_or(0))
        + (std::fs::metadata(&instrumental_compressed_path)
            .map(|m| m.len())
            .unwrap_or(0))
        + (std::fs::metadata(&instrumental_wav_path)
            .map(|m| m.len())
            .unwrap_or(0));

    let smid = message_id;

    log_trace(trace_id, "send_vocals_compressed", "");
    let p = SendAudioParams::builder()
        .chat_id(chat_id)
        .audio(PathBuf::from(&vocals_compressed_path))
        .caption(t("separation.result.vocals_compressed_caption"))
        .build();
    match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
        api,
        "sendAudio",
        &p,
        &vocals_compressed_path,
        chat_id,
        smid,
        "transfer.stage.sending_audio",
        None,
    )
    .await
    {
        Ok(_) => log_trace(trace_id, "vocals_compressed_sent", ""),
        Err(e) => log_trace(trace_id, "vocals_compressed_failed", &format!("err={e}")),
    }

    log_trace(trace_id, "send_vocals_wav", "");
    let p = frankenstein::methods::SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(PathBuf::from(&vocals_wav_path))
        .caption(t("separation.result.vocals_wav_caption"))
        .build();
    match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
        api,
        "sendDocument",
        &p,
        &vocals_wav_path,
        chat_id,
        smid,
        "transfer.stage.sending_document",
        None,
    )
    .await
    {
        Ok(_) => log_trace(trace_id, "vocals_wav_sent", ""),
        Err(e) => log_trace(trace_id, "vocals_wav_failed", &format!("err={e}")),
    }

    log_trace(trace_id, "send_instrumental_compressed", "");
    let p = SendAudioParams::builder()
        .chat_id(chat_id)
        .audio(PathBuf::from(&instrumental_compressed_path))
        .caption(t("separation.result.instrumental_compressed_caption"))
        .build();
    match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
        api,
        "sendAudio",
        &p,
        &instrumental_compressed_path,
        chat_id,
        smid,
        "transfer.stage.sending_audio",
        None,
    )
    .await
    {
        Ok(_) => log_trace(trace_id, "instrumental_compressed_sent", ""),
        Err(e) => log_trace(
            trace_id,
            "instrumental_compressed_failed",
            &format!("err={e}"),
        ),
    }

    log_trace(trace_id, "send_instrumental_wav", "");
    let p = frankenstein::methods::SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(PathBuf::from(&instrumental_wav_path))
        .caption(t("separation.result.instrumental_wav_caption"))
        .build();
    match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
        api,
        "sendDocument",
        &p,
        &instrumental_wav_path,
        chat_id,
        smid,
        "transfer.stage.sending_document",
        None,
    )
    .await
    {
        Ok(_) => log_trace(trace_id, "instrumental_wav_sent", ""),
        Err(e) => log_trace(trace_id, "instrumental_wav_failed", &format!("err={e}")),
    }

    let up_elapsed = up_start.elapsed();
    let up_speed = if up_elapsed.as_secs_f64() > 0.0 {
        total_out_bytes as f64 / up_elapsed.as_secs_f64()
    } else {
        0.0
    };
    if let Some(jid) = stats_job_id {
        crate::stats::record_upload_done(
            jid,
            user_id,
            total_out_bytes as i64,
            Some(up_speed as i64),
            Some(4),
        )
        .await;
    }

    // Quota was already reserved via ffprobe duration; no second deduction needed.

    crate::stats::record_event_user(
        user_id,
        "separation",
        mode_label,
        "ok",
        result.duration_seconds.ceil() as i64,
    )
    .await;
    crate::metrics::get()
        .separation_requests_total
        .with_label_values(&["success"])
        .inc();

    // Final report and reset flow to separation state for back-to-back audio requests.
    let report = tf(
        "separation.done_report",
        &[
            ("duration", &format_clock(audio_duration_secs)),
            ("total", &format_clock(op_started.elapsed().as_secs())),
        ],
    );
    let text = format!("{report}\n\n{}", t("separation.send_audio_prompt"));
    let entities = entities_for_text(&text);
    flow_manager.set(user_id, FlowState::AwaitingSeparation);
    let mut params = frankenstein::methods::SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(
            prompt_keyboard(0),
        ))
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    let _ = api.send_message(&params).await;

    std::fs::remove_dir_all(tmp_dir).ok();
    log_trace(trace_id, "cleanup_done", "");
}
