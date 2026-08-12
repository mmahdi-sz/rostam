use std::path::PathBuf;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{SendAudioParams, SendDocumentParams, SendMessageParams, SendVideoParams},
};

use crate::i18n::{t, tf};

use super::super::trace::log_trace;
use super::progress::format_upload_body;
use super::status::{edit_progress_status, edit_status};

pub enum MediaPayload {
    Video(SendVideoParams),
    Document(SendDocumentParams),
}

impl MediaPayload {
    #[allow(dead_code)]
    pub fn file_path(&self) -> Option<&std::path::Path> {
        match self {
            MediaPayload::Video(p) => match &p.video {
                FileUpload::InputFile(i) => Some(&i.path),
                _ => None,
            },
            MediaPayload::Document(p) => match &p.document {
                FileUpload::InputFile(i) => Some(&i.path),
                _ => None,
            },
        }
    }
}

/// Runs the send_video or send_document call with progress ticks and cancel support.
/// Returns `true` on success, `false` if cancelled or failed.
pub async fn send_media_with_progress(
    api: &Bot,
    payload: MediaPayload,
    chat_id: i64,
    status_chat_id: i64,
    status_message_id: i32,
    request_id: u64,
    quality_label: &str,
    cancel_fut: &mut std::pin::Pin<&mut impl std::future::Future<Output = ()>>,
    trace_id: u64,
) -> bool {
    let file_bytes = match &payload {
        MediaPayload::Video(p) => match &p.video {
            FileUpload::InputFile(i) => std::fs::metadata(&i.path).map(|m| m.len()).unwrap_or(0),
            _ => 0,
        },
        MediaPayload::Document(p) => match &p.document {
            FileUpload::InputFile(i) => std::fs::metadata(&i.path).map(|m| m.len()).unwrap_or(0),
            _ => 0,
        },
    };
    let progress = crate::bot::transfer::TransferProgress::new(file_bytes);
    let progress_clone = progress.clone();
    let api_url = api.api_url.clone();
    let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel_clone = cancel_flag.clone();
    
    let mut send_task = tokio::spawn(async move {
        match payload {
            MediaPayload::Video(params) => crate::bot::transfer::send_params_metered::<_, frankenstein::response::MethodResponse<frankenstein::types::Message>>(
                &api_url, "sendVideo", &params, &progress_clone, Some(cancel_clone)
            ).await.map(|_| ()),
            MediaPayload::Document(params) => crate::bot::transfer::send_params_metered::<_, frankenstein::response::MethodResponse<frankenstein::types::Message>>(
                &api_url, "sendDocument", &params, &progress_clone, Some(cancel_clone)
            ).await.map(|_| ()),
        }
    });

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3));
    interval.tick().await;
    let mut last_snap_str = String::new();

    let send_result = loop {
        tokio::select! {
            result = &mut send_task => { break result; }
            _ = interval.tick() => {
                let snap = progress.snapshot();
                let body = format_upload_body(quality_label, &snap);
                if body != last_snap_str {
                    last_snap_str = body.clone();
                    edit_progress_status(api, status_chat_id, status_message_id, body, request_id).await;
                }
            }
            _ = cancel_fut.as_mut() => {
                log_trace(trace_id, "upload_cancelled", "cancel signal");
                cancel_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                send_task.abort();
                edit_status(api, status_chat_id, status_message_id, t("youtube.download.cancelled")).await;
                return false;
            }
        }
    };

    match send_result {
        Ok(Ok(_)) => {
            log_trace(
                trace_id,
                "upload_ok",
                &format!("elapsed={}s", progress.elapsed().as_secs()),
            );
            true
        }
        Ok(Err(e)) => {
            log_trace(trace_id, "upload_failed", &e.to_string());
            let _ = api
                .send_message(
                    &SendMessageParams::builder()
                        .chat_id(chat_id)
                        .text(tf(
                            "youtube.download.upload_failed",
                            &[("error", &e.to_string())],
                        ))
                        .build(),
                )
                .await;
            false
        }
        Err(e) => {
            log_trace(trace_id, "upload_join_failed", &e.to_string());
            false
        }
    }
}

/// Runs the send_video call with progress ticks and cancel support.
/// Returns `true` on success, `false` if cancelled or failed.
#[allow(dead_code)]
pub async fn send_video_with_progress(
    api: &Bot,
    params: SendVideoParams,
    chat_id: i64,
    status_chat_id: i64,
    status_message_id: i32,
    request_id: u64,
    quality_label: &str,
    cancel_fut: &mut std::pin::Pin<&mut impl std::future::Future<Output = ()>>,
    trace_id: u64,
) -> bool {
    send_media_with_progress(
        api,
        MediaPayload::Video(params),
        chat_id,
        status_chat_id,
        status_message_id,
        request_id,
        quality_label,
        cancel_fut,
        trace_id,
    )
    .await
}

pub async fn send_audio_file(
    api: &Bot,
    chat_id: i64,
    path: &str,
    title: String,
    performer: String,
    caption: String,
    caption_entities: Vec<frankenstein::types::MessageEntity>,
    status_chat_id: i64,
    status_message_id: i32,
    request_id: u64,
    cancel_fut: &mut std::pin::Pin<&mut impl std::future::Future<Output = ()>>,
    trace_id: u64,
) -> bool {
    let mut params = SendAudioParams::builder()
        .chat_id(chat_id)
        .audio(FileUpload::InputFile(InputFile {
            path: PathBuf::from(path),
        }))
        .caption(caption)
        .build();
    if !title.is_empty() {
        params.title = Some(title);
    }
    if !performer.is_empty() {
        params.performer = Some(performer);
    }
    if !caption_entities.is_empty() {
        params.caption_entities = Some(caption_entities);
    }
    
    let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let progress = crate::bot::transfer::TransferProgress::new(file_bytes);
    let progress_clone = progress.clone();
    let api_url = api.api_url.clone();
    let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel_clone = cancel_flag.clone();
    
    let mut send_task = tokio::spawn(async move {
        crate::bot::transfer::send_params_metered::<_, frankenstein::response::MethodResponse<frankenstein::types::Message>>(
            &api_url, "sendAudio", &params, &progress_clone, Some(cancel_clone)
        ).await.map(|_| ())
    });
    
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3));
    interval.tick().await;
    let label = t("youtube.audio.uploading");
    let mut last_snap_str = String::new();
    
    let send_result = loop {
        tokio::select! {
            result = &mut send_task => { break result; }
            _ = interval.tick() => {
                let snap = progress.snapshot();
                let body = format_upload_body(&label, &snap);
                if body != last_snap_str {
                    last_snap_str = body.clone();
                    edit_progress_status(api, status_chat_id, status_message_id, body, request_id).await;
                }
            }
            _ = cancel_fut.as_mut() => {
                log_trace(trace_id, "audio_upload_cancelled", "cancel signal");
                cancel_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                send_task.abort();
                edit_status(api, status_chat_id, status_message_id, t("youtube.download.cancelled")).await;
                return false;
            }
        }
    };

    match send_result {
        Ok(Ok(_)) => {
            log_trace(
                trace_id,
                "audio_upload_ok",
                &format!("elapsed={}s", progress.elapsed().as_secs()),
            );
            true
        }
        Ok(Err(e)) => {
            log_trace(trace_id, "audio_upload_failed", &e.to_string());
            crate::stats::record_error_global("youtube", &format!("audio_upload_failed: {e}"))
                .await;
            let _ = api
                .send_message(
                    &SendMessageParams::builder()
                        .chat_id(chat_id)
                        .text(tf(
                            "youtube.download.upload_failed",
                            &[("error", &e.to_string())],
                        ))
                        .build(),
                )
                .await;
            false
        }
        Err(e) => {
            log_trace(trace_id, "audio_upload_join_failed", &e.to_string());
            crate::stats::record_error_global("youtube", &format!("audio_upload_join_failed: {e}"))
                .await;
            false
        }
    }
}

pub fn build_single_params(
    path: &str,
    chat_id: i64,
    thumb_path: &Option<String>,
    caption: String,
    caption_entities: Vec<frankenstein::types::MessageEntity>,
    height: u32,
    duration: Option<u64>,
) -> SendVideoParams {
    let mut params = SendVideoParams::builder()
        .chat_id(chat_id)
        .video(FileUpload::InputFile(InputFile {
            path: PathBuf::from(path),
        }))
        .supports_streaming(true)
        .caption(caption)
        .build();
    if !caption_entities.is_empty() {
        params.caption_entities = Some(caption_entities);
    }
    if let Some(tp) = thumb_path {
        params.thumbnail = Some(FileUpload::InputFile(InputFile {
            path: PathBuf::from(tp),
        }));
    }
    if let Some(d) = duration {
        if d > 0 && d <= u32::MAX as u64 {
            params.duration = Some(d as u32);
        }
    }
    params.height = Some(height);
    params.width = Some(height * 16 / 9);
    params
}

pub fn build_part_params(
    part_path: &str,
    chat_id: i64,
    thumb_path: &Option<String>,
    caption: String,
    caption_entities: Vec<frankenstein::types::MessageEntity>,
    height: u32,
) -> SendVideoParams {
    let mut params = SendVideoParams::builder()
        .chat_id(chat_id)
        .video(FileUpload::InputFile(InputFile {
            path: PathBuf::from(part_path),
        }))
        .supports_streaming(true)
        .caption(caption)
        .build();
    if !caption_entities.is_empty() {
        params.caption_entities = Some(caption_entities);
    }
    if let Some(tp) = thumb_path {
        params.thumbnail = Some(FileUpload::InputFile(InputFile {
            path: PathBuf::from(tp),
        }));
    }
    params.height = Some(height);
    params.width = Some(height * 16 / 9);
    params
}

pub fn build_single_doc_params(
    path: &str,
    chat_id: i64,
    thumb_path: &Option<String>,
    caption: String,
    caption_entities: Vec<frankenstein::types::MessageEntity>,
) -> SendDocumentParams {
    let mut params = SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(FileUpload::InputFile(InputFile {
            path: PathBuf::from(path),
        }))
        .caption(caption)
        .build();
    if !caption_entities.is_empty() {
        params.caption_entities = Some(caption_entities);
    }
    if let Some(tp) = thumb_path {
        params.thumbnail = Some(FileUpload::InputFile(InputFile {
            path: PathBuf::from(tp),
        }));
    }
    params
}

pub fn build_part_doc_params(
    part_path: &str,
    chat_id: i64,
    thumb_path: &Option<String>,
    caption: String,
    caption_entities: Vec<frankenstein::types::MessageEntity>,
) -> SendDocumentParams {
    let mut params = SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(FileUpload::InputFile(InputFile {
            path: PathBuf::from(part_path),
        }))
        .caption(caption)
        .build();
    if !caption_entities.is_empty() {
        params.caption_entities = Some(caption_entities);
    }
    if let Some(tp) = thumb_path {
        params.thumbnail = Some(FileUpload::InputFile(InputFile {
            path: PathBuf::from(tp),
        }));
    }
    params
}
