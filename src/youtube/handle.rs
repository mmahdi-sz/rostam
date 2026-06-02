use std::collections::HashSet;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::FileUpload,
    methods::{SendMessageParams, SendPhotoParams},
    types::LinkPreviewOptions,
};

use crate::bot::send_text;
use crate::cookie_pool::{CookiePool, format_no_cookie_available, save_snapshot};
use crate::database::postgresql::PostgresDatabase;
use crate::i18n::tf;

use super::format::{build_caption, build_description_blockquotes};
use super::fetch::fetch_video_info;
use super::quality_keyboard::send_quality_prompt;
use super::trace::log_trace;
use super::types::FetchError;

pub async fn handle_youtube_url(
    api: &Bot,
    chat_id: i64,
    user_id: Option<i64>,
    trace_id: u64,
    url: &str,
    cookie_pool: &mut CookiePool,
    database: &Option<PostgresDatabase>,
) {
    log_trace(trace_id, "handle_start", &format!("user_id={user_id:?} chat_id={chat_id} url={url}"));
    let mut tried: HashSet<String> = HashSet::new();
    loop {
        let cookie = match cookie_pool.next_cookie() {
            Some(c) => c,
            None => {
                let status = cookie_pool.status();
                log_trace(trace_id, "cookie_none", &format!("status selectable={} cooldown={}", status.selectable_cookies, status.cooldown_cookies));
                let _ = send_text(api, chat_id, &format_no_cookie_available(&status)).await;
                return;
            }
        };
        if tried.contains(&cookie.id) {
            let status = cookie_pool.status();
            log_trace(trace_id, "cookie_retry_exhausted", &format!("tried={tried:?} selectable={} cooldown={}", status.selectable_cookies, status.cooldown_cookies));
            let _ = send_text(api, chat_id, &format_no_cookie_available(&status)).await;
            return;
        }
        tried.insert(cookie.id.clone());
        log_trace(trace_id, "cookie_selected", &format!("cookie_id={} profile={}", cookie.id, cookie.profile_name));
        save_snapshot(database, cookie_pool).await;

        match fetch_video_info(trace_id, url, &cookie.yt_dlp_browser_spec).await {
            Ok(info) => {
                log_trace(trace_id, "fetch_ok", &format!("title={:?} heights={:?} thumbnail={}", info.title, info.available_heights, info.thumbnail.is_some()));
                let caption = build_caption(&info);
                let photo = info.thumbnail.clone().unwrap_or_else(|| info.webpage_url.clone());
                let params = SendPhotoParams::builder()
                    .chat_id(chat_id)
                    .photo(FileUpload::String(photo))
                    .caption(caption)
                    .parse_mode(ParseMode::MarkdownV2)
                    .build();
                if let Err(error) = api.send_photo(&params).await {
                    eprintln!("send_photo failed: {error}");
                    log_trace(trace_id, "send_photo_failed", &error.to_string());
                    let _ = send_text(api, chat_id, &tf("youtube.send_photo_failed", &[("error", &error.to_string())])).await;
                    return;
                }
                log_trace(trace_id, "send_photo_ok", "preview photo sent");
                if let Some(desc) = info.description.as_deref() {
                    if desc.chars().count() > 1000 {
                        let link_preview = LinkPreviewOptions::builder().is_disabled(true).build();
                        let chunks = build_description_blockquotes(desc);
                        log_trace(trace_id, "description_chunks", &format!("count={}", chunks.len()));
                        for chunk in chunks {
                            let msg = SendMessageParams::builder()
                                .chat_id(chat_id)
                                .text(chunk)
                                .parse_mode(ParseMode::MarkdownV2)
                                .link_preview_options(link_preview.clone())
                                .build();
                            if let Err(error) = api.send_message(&msg).await {
                                eprintln!("send description chunk failed: {error}");
                                log_trace(trace_id, "description_chunk_failed", &error.to_string());
                                break;
                            }
                        }
                    }
                }
                if let Err(error) = send_quality_prompt(
                    trace_id,
                    api,
                    chat_id,
                    user_id,
                    &cookie.yt_dlp_browser_spec,
                    &info,
                )
                .await
                {
                    eprintln!("send quality prompt failed: {error}");
                    log_trace(trace_id, "quality_prompt_failed", &error.to_string());
                    let _ = send_text(api, chat_id, &tf("youtube.quality.send_failed", &[("error", &error.to_string())])).await;
                }
                return;
            }
            Err(FetchError::RateLimited) => {
                if cookie_pool.mark_last_rate_limited() == Some(true) {
                    save_snapshot(database, cookie_pool).await;
                }
                eprintln!("yt-dlp 429 with cookie {}; retrying", cookie.id);
                log_trace(trace_id, "fetch_rate_limited", &format!("cookie_id={}", cookie.id));
                continue;
            }
            Err(FetchError::BadCookie(msg)) => {
                eprintln!("bad cookie {}: {msg}; trying next", cookie.id);
                log_trace(trace_id, "fetch_bad_cookie", &format!("cookie_id={} error={msg}", cookie.id));
                continue;
            }
            Err(FetchError::Other(msg)) => {
                eprintln!("yt-dlp failed for {url}: {msg}");
                log_trace(trace_id, "fetch_failed", &msg);
                let _ = send_text(api, chat_id, &tf("youtube.fetch_failed", &[("error", &msg)])).await;
                return;
            }
        }
    }
}
