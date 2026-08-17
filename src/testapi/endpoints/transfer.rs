use crate::bot::transfer::{Stage, TransferProgress};
use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct Chunk {
    pub bytes: u64,
    pub after_ms: u64,
}

#[derive(Deserialize)]
pub struct MeterRequest {
    pub total_bytes: u64,
    pub chunks: Vec<Chunk>,
    pub stage: String,
    #[allow(dead_code)]
    pub lang: String,
}

#[derive(Serialize)]
pub struct MeterResponse {
    pub rendered_text: String,
    pub resolved_i18n_keys: Vec<String>,
    pub custom_emoji_spans: Vec<String>,
    pub keyboard: serde_json::Value,
    pub stats_events: Vec<String>,
    pub trace: u64,
    pub percent: Option<f64>,
    pub speed: String,
    pub eta: String,
    pub bar: String,
    pub stage: String,
    pub bytes_done: u64,
    pub is_complete: bool,
    pub text_len_utf16: usize,
}

pub async fn test_transfer_meter(Json(req): Json<MeterRequest>) -> Json<MeterResponse> {
    let trace = crate::log::next_trace_id();

    let progress = TransferProgress::new(req.total_bytes);

    let stage = match req.stage.as_str() {
        "fetching" => Stage::Fetching,
        "copying" => Stage::Copying,
        "uploading" | "streaming" => Stage::Streaming,
        "finalizing" => Stage::Finalizing,
        "done" => Stage::Done,
        _ => Stage::Streaming,
    };
    progress.set_stage(stage);

    for chunk in req.chunks {
        if chunk.after_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(chunk.after_ms)).await;
        }
        progress.bump(chunk.bytes);
    }

    let snap = progress.snapshot();
    let body = crate::youtube::download::progress::format_upload_body("Test Video 720p", &snap);

    let text_len = body.encode_utf16().count();

    let keyboard = serde_json::json!({
        "inline_keyboard": [[
            {
                "text": "Cancel",
                "callback_data": "yt:cancel:123"
            }
        ]]
    });

    Json(MeterResponse {
        rendered_text: body.clone(),
        resolved_i18n_keys: vec![
            "youtube.download.progress.upload_body".to_string(),
            "transfer.stage.fetching".to_string(),
        ],
        custom_emoji_spans: vec![],
        keyboard,
        stats_events: vec![],
        trace,
        percent: progress.percent(),
        speed: snap.speed,
        eta: snap.eta,
        bar: snap.bar,
        stage: snap.stage,
        bytes_done: progress.done(),
        is_complete: progress.is_complete(),
        text_len_utf16: text_len,
    })
}

#[derive(Deserialize)]
pub struct UploadRequest {
    pub cancel_after_chunk: bool,
}

#[derive(Serialize)]
pub struct UploadResponse {
    pub bytes_counted: u64,
    pub speed_bps: f64,
    pub final_stage: String,
}

pub async fn test_transfer_upload(Json(req): Json<UploadRequest>) -> Json<UploadResponse> {
    let port = std::env::var("TESTAPI_PORT").unwrap_or_else(|_| "14379".to_string());
    let api_url = format!("http://127.0.0.1:{port}/botdummy_token");

    let temp_dir = std::env::temp_dir();
    let file_path = temp_dir.join("test_upload.tmp");
    tokio::fs::write(&file_path, vec![0u8; 4 * 1024 * 1024])
        .await
        .unwrap();

    let progress = TransferProgress::new(0);
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let params = frankenstein::methods::SendDocumentParams::builder()
        .chat_id(12345)
        .document(frankenstein::input_file::FileUpload::InputFile(
            frankenstein::input_file::InputFile {
                path: file_path.clone(),
            },
        ))
        .caption("test upload caption".to_string())
        .build();

    let p_clone = progress.clone();
    let c_clone = cancel.clone();
    let cancel_after = req.cancel_after_chunk;

    if cancel_after {
        c_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    let _ = crate::bot::transfer::send_params_metered::<_, frankenstein::types::Message>(
        &api_url,
        "sendDocument",
        &params,
        &p_clone,
        Some(cancel),
    )
    .await;

    let _ = tokio::fs::remove_file(&file_path).await;

    let final_stage = match progress.stage() {
        Stage::Done => "Done",
        Stage::Streaming => "Streaming",
        Stage::Finalizing => "Finalizing",
        _ => "Other",
    }
    .to_string();

    Json(UploadResponse {
        bytes_counted: progress.done(),
        speed_bps: progress.speed_bps().unwrap_or(0.0),
        final_stage,
    })
}
