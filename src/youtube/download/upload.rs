use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{
        DeleteMessageParams, SendAudioParams, SendDocumentParams, SendMessageParams,
        SendVideoParams,
    },
};

use crate::i18n::t;

use super::super::trace::log_trace;
use super::status::edit_status;

pub enum MediaPayload {
    Video(SendVideoParams),
    Document(SendDocumentParams),
}

enum UploadFailure {
    Cancelled,
    Failed(anyhow::Error),
}

async fn wait_upload_or_cancel<F, T>(
    send: F,
    cancel_fut: &mut std::pin::Pin<&mut impl std::future::Future<Output = ()>>,
    cancel_flag: Arc<AtomicBool>,
) -> (Result<T, UploadFailure>, bool)
where
    F: std::future::Future<Output = anyhow::Result<T>>,
{
    tokio::pin!(send);
    tokio::select! {
        biased;
        _ = cancel_fut.as_mut() => {
            cancel_flag.store(true, Ordering::SeqCst);
            let result = match send.await {
                Ok(message) => Ok(message),
                Err(_) => Err(UploadFailure::Cancelled),
            };
            (result, true)
        }
        result = &mut send => (result.map_err(UploadFailure::Failed), false),
    }
}

async fn classify_upload_result(
    api: &Bot,
    chat_id: i64,
    result: Result<frankenstein::types::Message, UploadFailure>,
    cancelled: bool,
    trace_id: u64,
) -> Result<(), UploadFailure> {
    if cancelled {
        if let Ok(message) = result {
            let params = DeleteMessageParams::builder()
                .chat_id(chat_id)
                .message_id(message.message_id)
                .build();
            match api.delete_message(&params).await {
                Ok(_) => log_trace(trace_id, "cancelled_upload_deleted", "sent message removed"),
                Err(e) => log_trace(trace_id, "cancelled_upload_delete_failed", &e.to_string()),
            }
        }
        Err(UploadFailure::Cancelled)
    } else {
        result.map(|_| ())
    }
}

#[cfg(feature = "testapi")]
pub fn test_upload_cancel_keyboard(request_id: u64) -> frankenstein::types::InlineKeyboardMarkup {
    super::status::cancel_keyboard(request_id)
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
    part: Option<(usize, usize)>,
    cancel_fut: &mut std::pin::Pin<&mut impl std::future::Future<Output = ()>>,
    trace_id: u64,
) -> bool {
    let context = |file_key| crate::bot::transfer::UploadStatusContext {
        chat_id: status_chat_id,
        message_id: status_message_id,
        file_key,
        description: Some(quality_label),
        part,
        keyboard: Some(super::status::cancel_keyboard(request_id)),
    };
    let result = match payload {
        MediaPayload::Video(params) => {
            let path = match &params.video {
                FileUpload::InputFile(i) => &i.path,
                _ => return false,
            };
            let cancel_flag = Arc::new(AtomicBool::new(false));
            let send = crate::bot::transfer::send_file_with_upload_context::<
                _,
                frankenstein::types::Message,
            >(
                api,
                "sendVideo",
                &params,
                path,
                context("transfer.stage.sending_video"),
                Some(cancel_flag.clone()),
            );
            let (result, cancelled) = wait_upload_or_cancel(send, cancel_fut, cancel_flag).await;
            classify_upload_result(api, chat_id, result, cancelled, trace_id).await
        }
        MediaPayload::Document(params) => {
            let path = match &params.document {
                FileUpload::InputFile(i) => &i.path,
                _ => return false,
            };
            let cancel_flag = Arc::new(AtomicBool::new(false));
            let send = crate::bot::transfer::send_file_with_upload_context::<
                _,
                frankenstein::types::Message,
            >(
                api,
                "sendDocument",
                &params,
                path,
                context("transfer.stage.sending_document"),
                Some(cancel_flag.clone()),
            );
            let (result, cancelled) = wait_upload_or_cancel(send, cancel_fut, cancel_flag).await;
            classify_upload_result(api, chat_id, result, cancelled, trace_id).await
        }
    };
    match result {
        Ok(()) => {
            log_trace(trace_id, "upload_ok", "sent");
            true
        }
        Err(UploadFailure::Cancelled) => {
            log_trace(trace_id, "upload_cancelled", "cancelled");
            edit_status(
                api,
                status_chat_id,
                status_message_id,
                t("youtube.download.cancelled"),
            )
            .await;
            false
        }
        Err(UploadFailure::Failed(e)) => {
            log_trace(trace_id, "upload_failed", &e.to_string());
            let _ = api
                .send_message(
                    &SendMessageParams::builder()
                        .chat_id(chat_id)
                        .text(t("youtube.download.upload_failed"))
                        .build(),
                )
                .await;
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
        None,
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

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let send = crate::bot::transfer::send_file_with_upload_context::<_, frankenstein::types::Message>(
        api,
        "sendAudio",
        &params,
        std::path::Path::new(path),
        crate::bot::transfer::UploadStatusContext {
            chat_id: status_chat_id,
            message_id: status_message_id,
            file_key: "transfer.stage.sending_audio",
            description: params.title.as_deref(),
            part: None,
            keyboard: Some(super::status::cancel_keyboard(request_id)),
        },
        Some(cancel_flag.clone()),
    );
    let (result, cancelled) = wait_upload_or_cancel(send, cancel_fut, cancel_flag).await;
    let result = classify_upload_result(api, chat_id, result, cancelled, trace_id).await;
    match result {
        Ok(_) => {
            log_trace(trace_id, "audio_upload_ok", "sent");
            true
        }
        Err(UploadFailure::Cancelled) => {
            log_trace(trace_id, "audio_upload_cancelled", "cancelled");
            edit_status(
                api,
                status_chat_id,
                status_message_id,
                t("youtube.download.cancelled"),
            )
            .await;
            false
        }
        Err(UploadFailure::Failed(e)) => {
            log_trace(trace_id, "audio_upload_failed", &e.to_string());
            crate::stats::record_error_global("youtube", &format!("audio_upload_failed: {e}"))
                .await;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Notify;

    #[tokio::test]
    async fn notify_cancel_sets_transfer_flag_and_waits_for_send_to_stop() {
        let notify = Arc::new(Notify::new());
        let mut cancel_fut = std::pin::pin!(notify.notified());
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let flag_for_send = cancel_flag.clone();
        let send = async move {
            loop {
                if flag_for_send.load(Ordering::SeqCst) {
                    return Err::<(), anyhow::Error>(anyhow::anyhow!("upload cancelled"));
                }
                tokio::task::yield_now().await;
            }
        };
        let notify_for_cancel = notify.clone();
        let cancel_task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            notify_for_cancel.notify_one();
        });

        let (result, cancelled) =
            wait_upload_or_cancel(send, &mut cancel_fut, cancel_flag.clone()).await;
        assert!(matches!(result, Err(UploadFailure::Cancelled)));
        assert!(cancelled);
        assert!(cancel_flag.load(Ordering::SeqCst));
        cancel_task.await.unwrap();
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
