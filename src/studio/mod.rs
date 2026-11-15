//! Photo & Video Magic Studio (`studio`) top-level module.
//!
//! Houses media editing tools starting with video trimming (`studio_trim`).

pub mod burn;
pub mod compress;
pub mod extract;
pub mod pipeline;
pub mod trim;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, ReplyMarkup},
};

use crate::bot::constants::{
    CB_START_PANEL, CB_START_STUDIO, CB_STUDIO_BURN, CB_STUDIO_BURN_CANCEL,
    CB_STUDIO_BURN_JOBCANCEL, CB_STUDIO_COMPRESS, CB_STUDIO_EXTRACT, CB_STUDIO_EXTRACT_CANCEL,
    CB_STUDIO_EXTRACT_JOBCANCEL, CB_STUDIO_PANEL, CB_STUDIO_TRIM, CB_STUDIO_TRIM_CANCEL,
    CB_STUDIO_TRIM_JOBCANCEL,
};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::panel::{btn_icon, btn_icon_danger, btn_icon_primary};
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t};
use crate::log::next_trace_id;

pub fn studio_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon(
                &t("studio.trim_button"),
                CB_STUDIO_TRIM,
                "scissors",
            )],
            vec![btn_icon(
                &t("studio.compress_button"),
                CB_STUDIO_COMPRESS,
                "adobe_pr_animasion",
            )],
            vec![btn_icon(
                &t("studio.extract_button"),
                CB_STUDIO_EXTRACT,
                "sound_wave",
            )],
            vec![btn_icon(
                &t("studio.burn_button"),
                CB_STUDIO_BURN,
                "clapper",
            )],
            vec![btn_icon_primary(&t("start.back"), CB_START_PANEL, "back")],
        ])
        .build()
}

/// Enters the Photo & Video Magic Studio submenu.
pub async fn enter_studio(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.clear(user_id);
    log_actor_id!("studio", trace_id, user_id, "clicked" => CB_START_STUDIO);

    let text = apply_premium_to_md(&t("studio.title"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(studio_keyboard())
        .build();

    let _ = api.edit_message_text(&params).await;
}

/// Sends the Photo & Video Magic Studio menu as a new message.
pub async fn send_studio_menu_new_msg(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.clear(user_id);
    log_actor_id!("studio", trace_id, user_id, "rearm" => "studio_menu");

    let text = apply_premium_to_md(&t("studio.title"));
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(studio_keyboard()))
        .build();

    let _ = api.send_message(&params).await;
}

/// Enters the Video Trim & Edit prompt, setting `FlowState::AwaitingStudioTrimVideo`.
pub async fn enter_trim_prompt(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
    log_actor_id!("studio_trim", trace_id, user_id, "clicked" => CB_STUDIO_TRIM);

    let text = apply_premium_to_md(&t("studio.trim.send_video_prompt"));
    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("studio.trim.cancel_btn"),
            CB_STUDIO_TRIM_CANCEL,
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

/// Handles Studio callback queries.
pub async fn handle_callback(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    cb_data: &str,
    flow_manager: &FlowManager,
    database: &Option<PostgresDatabase>,
) -> bool {
    let trace_id = next_trace_id();
    log_ev!("studio", trace_id, "callback", "cb" => cb_data, "user_id" => user_id);

    if cb_data == CB_START_STUDIO || cb_data == CB_STUDIO_PANEL {
        enter_studio(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data == CB_STUDIO_COMPRESS {
        compress::enter_compress_prompt(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data.starts_with("stc:") {
        compress::handle_compress_cb(api, chat_id, message_id, user_id, cb_data, flow_manager).await
    } else if cb_data == CB_STUDIO_TRIM {
        enter_trim_prompt(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data == CB_STUDIO_TRIM_CANCEL {
        log_ev!("studio_trim", trace_id, "cancel_flow", "user_id" => user_id);
        enter_studio(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data == CB_STUDIO_TRIM_JOBCANCEL {
        log_ev!("studio_trim", trace_id, "job_cancel_clicked", "user_id" => user_id);
        let cancelled = pipeline::cancel_active_job(user_id);
        log_ev!("studio_trim", trace_id, "job_cancel_result", "cancelled" => cancelled);
        true
    } else if cb_data == CB_STUDIO_EXTRACT {
        extract::enter_extract_prompt(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data == CB_STUDIO_EXTRACT_CANCEL {
        log_ev!("studio_extract", trace_id, "cancel_flow", "user_id" => user_id);
        enter_studio(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data == CB_STUDIO_EXTRACT_JOBCANCEL {
        log_ev!("studio_extract", trace_id, "job_cancel_clicked", "user_id" => user_id);
        let cancelled = pipeline::cancel_active_job(user_id);
        log_ev!("studio_extract", trace_id, "job_cancel_result", "cancelled" => cancelled);
        true
    } else if cb_data == CB_STUDIO_BURN {
        burn::enter_burn_prompt(api, chat_id, message_id, user_id, flow_manager, database).await;
        true
    } else if cb_data == CB_STUDIO_BURN_CANCEL {
        log_ev!("studio_burn", trace_id, "cancel_flow", "user_id" => user_id);
        if let FlowState::AwaitingStudioBurnInput { session } = flow_manager.get(user_id) {
            burn::abort_session(&session);
        }
        enter_studio(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data == CB_STUDIO_BURN_JOBCANCEL {
        log_ev!("studio_burn", trace_id, "job_cancel_clicked", "user_id" => user_id);
        let cancelled = pipeline::cancel_active_job(user_id);
        log_ev!("studio_burn", trace_id, "job_cancel_result", "cancelled" => cancelled);
        true
    } else if cb_data == crate::bot::constants::CB_STUDIO_TRIM_START
        || cb_data.starts_with("studio_trim:start")
    {
        log_ev!("studio_trim", trace_id, "start_ranges_prompt", "user_id" => user_id);
        let text = apply_premium_to_md(&t("studio.trim.ranges_prompt"));
        let params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(&text)
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(trim::cancel_keyboard())
            .build();
        if let Err(e) = api.edit_message_text(&params).await {
            log_ev!("studio_trim", trace_id, "start_ranges_prompt_failed", "=>" => format!("fail err={e}"));
        }
        true
    } else {
        false
    }
}

/// Validates whether an incoming Telegram update message represents a video file
/// based purely on available metadata (`message.video` or `message.document` mime_type/file_name)
/// without executing `getFile` or downloading any content.
pub fn is_video_message_metadata(msg: &frankenstein::types::Message) -> bool {
    if msg.video.is_some() || msg.animation.is_some() || msg.video_note.is_some() {
        return true;
    }
    if msg.audio.is_some() || msg.voice.is_some() {
        return false;
    }
    if let Some(doc) = &msg.document {
        // Priority 1: If document filename has a recognizable audio extension, it is audio, not video.
        // Critical: iOS/Android Telegram clients frequently send .m4a audio files with mime_type="video/mp4".
        if let Some(name) = &doc.file_name {
            if is_audio_filename(name) {
                return false;
            }
            if is_video_filename(name) {
                return true;
            }
        }
        if let Some(mime) = &doc.mime_type {
            let mime_lower = mime.to_lowercase();
            if mime_lower.starts_with("image/")
                || mime_lower.starts_with("audio/")
                || mime_lower.starts_with("text/")
                || mime_lower == "application/pdf"
                || mime_lower == "application/zip"
                || mime_lower == "application/x-rar-compressed"
                || mime_lower == "application/x-7z-compressed"
            {
                return false;
            }
            if mime_lower.starts_with("video/")
                || mime_lower == "application/octet-stream"
                || mime_lower == "binary/octet-stream"
                || mime_lower == "application/x-matroska"
                || mime_lower == "application/x-flash-video"
                || mime_lower == "application/mxf"
                || mime_lower == "application/vnd.rn-realmedia"
            {
                return true;
            }
        }
        if let Some(name) = &doc.file_name {
            let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
            if matches!(
                ext.as_str(),
                "pdf"
                    | "zip"
                    | "rar"
                    | "7z"
                    | "txt"
                    | "jpg"
                    | "jpeg"
                    | "png"
                    | "gif"
                    | "webp"
                    | "mp3"
                    | "wav"
                    | "flac"
                    | "m4a"
                    | "ogg"
                    | "opus"
                    | "aac"
            ) {
                return false;
            }
        }
        return true;
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TrimMediaKind {
    Video,
    Audio,
}

/// Checks if a file name has an audio file extension.
pub fn is_audio_filename(filename: &str) -> bool {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    matches!(
        ext.as_str(),
        "mp3" | "m4a" | "flac" | "ogg" | "opus" | "wav" | "aac" | "wma" | "alac" | "aiff"
    )
}

/// Checks if an audio file extension is officially guaranteed by Telegram Bot API for `sendAudio` (strictly MP3 and M4A).
pub fn is_telegram_audio_guaranteed(ext: &str) -> bool {
    matches!(ext.to_lowercase().as_str(), "mp3" | "m4a")
}

/// Checks if a file name has a video file extension.
pub fn is_video_filename(filename: &str) -> bool {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    matches!(
        ext.as_str(),
        "mp4"
            | "mkv"
            | "avi"
            | "mov"
            | "webm"
            | "flv"
            | "wmv"
            | "m4v"
            | "3gp"
            | "3g2"
            | "ts"
            | "mts"
            | "m2ts"
            | "vob"
            | "ogv"
            | "qt"
            | "f4v"
            | "asf"
            | "rm"
            | "rmvb"
            | "mpg"
            | "mpeg"
            | "mpe"
            | "mpv"
            | "divx"
            | "xvid"
            | "m2v"
            | "264"
            | "h264"
            | "265"
            | "h265"
            | "hevc"
            | "av1"
    )
}

/// Detects whether an incoming Telegram update message represents a video or audio file for trimming.
///
/// Recognizes native `video`, `animation`, `video_note`, and `audio`, `voice`, as well as
/// `document` uploads with corresponding audio/video MIME types or file extensions.
/// Returns `None` if the message contains unsupported document types or no media.
pub fn detect_trim_media(msg: &frankenstein::types::Message) -> Option<TrimMediaKind> {
    if msg.video.is_some() || msg.animation.is_some() || msg.video_note.is_some() {
        return Some(TrimMediaKind::Video);
    }
    if msg.audio.is_some() || msg.voice.is_some() {
        return Some(TrimMediaKind::Audio);
    }
    if let Some(doc) = &msg.document {
        // Priority 1: Check filename extension first.
        // Mobile Telegram clients (iOS/Android) frequently send .m4a audio files with mime_type="video/mp4".
        // Prioritizing is_audio_filename prevents misclassifying .m4a documents as Video.
        if let Some(name) = &doc.file_name {
            if is_audio_filename(name) {
                return Some(TrimMediaKind::Audio);
            }
            if is_video_filename(name) {
                return Some(TrimMediaKind::Video);
            }
        }

        // Priority 2: MIME type inspection.
        if let Some(mime) = &doc.mime_type {
            let mime_lower = mime.to_lowercase();
            if mime_lower.starts_with("audio/")
                || mime_lower == "application/ogg"
                || mime_lower == "application/x-flac"
                || mime_lower == "audio/mp4"
                || mime_lower == "audio/x-m4a"
                || mime_lower == "audio/m4a"
                || mime_lower == "audio/aac"
                || mime_lower == "audio/x-aac"
            {
                return Some(TrimMediaKind::Audio);
            }
            if mime_lower.starts_with("video/")
                || mime_lower == "application/x-matroska"
                || mime_lower == "application/x-flash-video"
                || mime_lower == "application/mxf"
                || mime_lower == "application/vnd.rn-realmedia"
            {
                return Some(TrimMediaKind::Video);
            }
        }

        // Priority 3: Fallback for generic octet-stream
        if let Some(mime) = &doc.mime_type {
            let mime_lower = mime.to_lowercase();
            if mime_lower == "application/octet-stream" || mime_lower == "binary/octet-stream" {
                if let Some(name) = &doc.file_name {
                    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
                    if matches!(
                        ext.as_str(),
                        "pdf"
                            | "zip"
                            | "rar"
                            | "7z"
                            | "txt"
                            | "jpg"
                            | "jpeg"
                            | "png"
                            | "gif"
                            | "webp"
                    ) {
                        return None;
                    }
                }
                return Some(TrimMediaKind::Video);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use frankenstein::types::Message;

    fn make_test_msg() -> Message {
        serde_json::from_str::<Message>(
            r#"{
                "message_id": 1,
                "date": 1000,
                "chat": {"id": 123, "type": "private"}
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn test_is_video_message_metadata() {
        let msg = make_test_msg();
        assert!(!is_video_message_metadata(&msg));

        // Test video message
        let video_msg: Message = serde_json::from_str(
            r#"{
                "message_id": 1,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "video": {
                    "file_id": "v1",
                    "file_unique_id": "u1",
                    "width": 1920,
                    "height": 1080,
                    "duration": 60,
                    "file_name": "clip.mp4",
                    "mime_type": "video/mp4",
                    "file_size": 1000
                }
            }"#,
        )
        .unwrap();
        assert!(is_video_message_metadata(&video_msg));

        // Test video document via mime_type
        let doc_mime_msg: Message = serde_json::from_str(
            r#"{
                "message_id": 2,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "document": {
                    "file_id": "d1",
                    "file_unique_id": "ud1",
                    "file_name": "file.bin",
                    "mime_type": "video/x-matroska",
                    "file_size": 2000
                }
            }"#,
        )
        .unwrap();
        assert!(is_video_message_metadata(&doc_mime_msg));

        // Test video document via file extension
        let doc_ext_msg: Message = serde_json::from_str(
            r#"{
                "message_id": 3,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "document": {
                    "file_id": "d2",
                    "file_unique_id": "ud2",
                    "file_name": "sample.mkv",
                    "mime_type": "application/octet-stream",
                    "file_size": 2000
                }
            }"#,
        )
        .unwrap();
        assert!(is_video_message_metadata(&doc_ext_msg));

        // Test non-video document (PDF)
        let pdf_msg: Message = serde_json::from_str(
            r#"{
                "message_id": 4,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "document": {
                    "file_id": "d3",
                    "file_unique_id": "ud3",
                    "file_name": "paper.pdf",
                    "mime_type": "application/pdf",
                    "file_size": 500
                }
            }"#,
        )
        .unwrap();
        assert!(!is_video_message_metadata(&pdf_msg));

        // Test audio document with video/mp4 MIME (standard iOS/Android behavior)
        let m4a_doc_msg: Message = serde_json::from_str(
            r#"{
                "message_id": 5,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "document": {
                    "file_id": "d4",
                    "file_unique_id": "ud4",
                    "file_name": "track.m4a",
                    "mime_type": "video/mp4",
                    "file_size": 4000
                }
            }"#,
        )
        .unwrap();
        assert!(!is_video_message_metadata(&m4a_doc_msg));
    }

    #[test]
    fn test_detect_trim_media() {
        // Native video
        let video_msg: Message = serde_json::from_str(
            r#"{
                "message_id": 10,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "video": {
                    "file_id": "v1",
                    "file_unique_id": "u1",
                    "width": 1920,
                    "height": 1080,
                    "duration": 60
                }
            }"#,
        )
        .unwrap();
        assert_eq!(detect_trim_media(&video_msg), Some(TrimMediaKind::Video));

        // Native audio
        let audio_msg: Message = serde_json::from_str(
            r#"{
                "message_id": 11,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "audio": {
                    "file_id": "a1",
                    "file_unique_id": "ua1",
                    "duration": 180,
                    "title": "Song Title",
                    "performer": "Artist"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(detect_trim_media(&audio_msg), Some(TrimMediaKind::Audio));

        // Native audio with mime_type: "audio/mp4"
        let audio_mp4_msg: Message = serde_json::from_str(
            r#"{
                "message_id": 111,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "audio": {
                    "file_id": "a2",
                    "file_unique_id": "ua2",
                    "duration": 200,
                    "mime_type": "audio/mp4"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            detect_trim_media(&audio_mp4_msg),
            Some(TrimMediaKind::Audio)
        );

        // Native voice
        let voice_msg: Message = serde_json::from_str(
            r#"{
                "message_id": 12,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "voice": {
                    "file_id": "vo1",
                    "file_unique_id": "uvo1",
                    "duration": 30
                }
            }"#,
        )
        .unwrap();
        assert_eq!(detect_trim_media(&voice_msg), Some(TrimMediaKind::Audio));

        // Audio document with video/mp4 MIME and track.m4a filename (critical iOS/Android case)
        let doc_m4a_video_mime: Message = serde_json::from_str(
            r#"{
                "message_id": 125,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "document": {
                    "file_id": "d_m4a",
                    "file_unique_id": "ud_m4a",
                    "file_name": "track.m4a",
                    "mime_type": "video/mp4"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            detect_trim_media(&doc_m4a_video_mime),
            Some(TrimMediaKind::Audio)
        );

        // Audio document with audio/mp4 MIME
        let doc_audio_mp4: Message = serde_json::from_str(
            r#"{
                "message_id": 126,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "document": {
                    "file_id": "d_amp4",
                    "file_unique_id": "ud_amp4",
                    "file_name": "unknown_file",
                    "mime_type": "audio/mp4"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            detect_trim_media(&doc_audio_mp4),
            Some(TrimMediaKind::Audio)
        );

        // Audio document with audio/x-m4a MIME
        let doc_audio_xm4a: Message = serde_json::from_str(
            r#"{
                "message_id": 127,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "document": {
                    "file_id": "d_axm4a",
                    "file_unique_id": "ud_axm4a",
                    "file_name": "audio_track",
                    "mime_type": "audio/x-m4a"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            detect_trim_media(&doc_audio_xm4a),
            Some(TrimMediaKind::Audio)
        );

        // Audio document via extension (e.g. mp3)
        let doc_mp3: Message = serde_json::from_str(
            r#"{
                "message_id": 13,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "document": {
                    "file_id": "d_mp3",
                    "file_unique_id": "ud_mp3",
                    "file_name": "song.mp3",
                    "mime_type": "application/octet-stream"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(detect_trim_media(&doc_mp3), Some(TrimMediaKind::Audio));

        // Audio document via audio/* MIME
        let doc_flac: Message = serde_json::from_str(
            r#"{
                "message_id": 14,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "document": {
                    "file_id": "d_flac",
                    "file_unique_id": "ud_flac",
                    "file_name": "track.flac",
                    "mime_type": "audio/flac"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(detect_trim_media(&doc_flac), Some(TrimMediaKind::Audio));

        // Video document with file_name: video.mp4 and mime_type: video/mp4
        let doc_mp4: Message = serde_json::from_str(
            r#"{
                "message_id": 15,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "document": {
                    "file_id": "d_mp4",
                    "file_unique_id": "ud_mp4",
                    "file_name": "video.mp4",
                    "mime_type": "video/mp4"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(detect_trim_media(&doc_mp4), Some(TrimMediaKind::Video));

        // Unsupported documents
        let doc_pdf: Message = serde_json::from_str(
            r#"{
                "message_id": 16,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "document": {
                    "file_id": "d_pdf",
                    "file_unique_id": "ud_pdf",
                    "file_name": "report.pdf",
                    "mime_type": "application/pdf"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(detect_trim_media(&doc_pdf), None);

        let doc_zip: Message = serde_json::from_str(
            r#"{
                "message_id": 17,
                "date": 1000,
                "chat": {"id": 123, "type": "private"},
                "document": {
                    "file_id": "d_zip",
                    "file_unique_id": "ud_zip",
                    "file_name": "archive.zip",
                    "mime_type": "application/zip"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(detect_trim_media(&doc_zip), None);
    }
}
