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
    #[serde(default = "default_file_key")]
    pub file_key: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub part: Option<(usize, usize)>,
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

fn default_file_key() -> String {
    "transfer.stage.sending_video".to_string()
}

fn custom_emoji_spans(body: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("![") {
        rest = &rest[start + 2..];
        let Some((label, tail)) = rest.split_once("](tg://emoji?id=") else {
            break;
        };
        let Some((id, after)) = tail.split_once(')') else {
            break;
        };
        spans.push(format!("{label} -> {id}"));
        rest = after;
    }
    spans
}

pub async fn test_transfer_meter(Json(req): Json<MeterRequest>) -> Json<MeterResponse> {
    let lang = req.lang.clone();
    let resolved_keys = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let keys_for_scope = resolved_keys.clone();
    crate::i18n::LANG
        .scope(lang, async move {
            crate::i18n::RESOLVED_I18N_KEYS
                .scope(keys_for_scope, async move {
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
                            tokio::time::sleep(std::time::Duration::from_millis(chunk.after_ms))
                                .await;
                        }
                        progress.bump(chunk.bytes);
                    }

                    let snap = progress.snapshot();
                    let body = crate::bot::transfer::render_upload_status(
                        &progress,
                        &crate::bot::transfer::UploadStatusContext {
                            chat_id: 12345,
                            message_id: 1,
                            file_key: &req.file_key,
                            description: req.description.as_deref(),
                            part: req.part,
                            keyboard: Some(crate::youtube::download::test_upload_cancel_keyboard(
                                123,
                            )),
                        },
                    );

                    let text_len = body.encode_utf16().count();
                    let custom_emoji_spans = custom_emoji_spans(&body);

                    let keyboard = serde_json::to_value(
                        crate::youtube::download::test_upload_cancel_keyboard(123),
                    )
                    .unwrap_or_else(|_| serde_json::json!({ "inline_keyboard": [] }));

                    Json(MeterResponse {
                        rendered_text: body.clone(),
                        resolved_i18n_keys: resolved_keys
                            .lock()
                            .map(|keys| keys.clone())
                            .unwrap_or_default(),
                        custom_emoji_spans,
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
                })
                .await
        })
        .await
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
    let cancel_task = req.cancel_after_chunk.then(|| {
        let cancel = cancel.clone();
        let progress = progress.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while progress.done() < 64 * 1024 {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            })
            .await;
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        })
    });

    let api = frankenstein::client_reqwest::Bot::new_url(api_url);
    let _ = crate::bot::transfer::send_file_with_upload_context_test::<
        _,
        frankenstein::types::Message,
    >(
        &api,
        "sendDocument",
        &params,
        &file_path,
        crate::bot::transfer::UploadStatusContext {
            chat_id: 12345,
            message_id: 0,
            file_key: "transfer.stage.sending_document",
            description: Some("test upload"),
            part: None,
            keyboard: None,
        },
        Some(cancel),
        p_clone,
    )
    .await;
    if let Some(task) = cancel_task {
        let _ = task.await;
    }

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
