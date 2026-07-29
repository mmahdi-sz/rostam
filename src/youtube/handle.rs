use std::collections::HashSet;
use std::sync::Arc;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::FileUpload,
    methods::{DeleteMessageParams, SendMessageParams, SendPhotoParams},
    types::{LinkPreviewOptions, ReplyParameters},
};
use tokio::sync::Mutex;

use crate::bot::send_text;
use crate::cookie_pool::{CookiePool, CookieSource, format_no_cookie_available, save_snapshot};
use crate::database::postgresql::PostgresDatabase;
use crate::i18n::{t, tf};

use super::fetch::fetch_video_info;
use super::format::{build_caption, build_description_blockquotes};
use super::quality_keyboard::send_quality_prompt;
use super::trace::log_trace;
use super::types::FetchError;

pub async fn handle_youtube_url(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: Option<i64>,
    trace_id: u64,
    url: &str,
    cookie_pool: Arc<Mutex<CookiePool>>,
    database: &Option<PostgresDatabase>,
    rate_limit_tx: &tokio::sync::mpsc::UnboundedSender<CookieSource>,
) -> crate::error::Result<()> {
    let Some(url_str) = crate::validation::sanitize_url(url) else {
        log_trace(trace_id, "invalid_url", "URL failed sanitization");
        let _ = send_text(api, chat_id, &t("surge.invalid_url")).await;
        anyhow::bail!("invalid url");
    };
    let url = &url_str;

    if let Some(uid) = user_id {
        log_actor_id!("yt", trace_id, uid, "clicked" => "url");
    }
    log_trace(
        trace_id,
        "handle_start",
        &format!("user_id={user_id:?} chat_id={chat_id} url={url}"),
    );

    let analyzing_text = t("youtube.analyzing");
    let entities = crate::i18n::entities_for_text(&analyzing_text);
    let mut analyzing_params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(analyzing_text)
        .reply_parameters(ReplyParameters::builder().message_id(message_id).build())
        .build();
    if !entities.is_empty() {
        analyzing_params.entities = Some(entities);
    }
    let analyzing_msg_id = match api.send_message(&analyzing_params).await {
        Ok(resp) => {
            let mid = resp.result.message_id;
            log_trace(
                trace_id,
                "analyzing_reply_sent",
                &format!("analyzing_msg_id={mid}"),
            );
            Some(mid)
        }
        Err(e) => {
            log_trace(trace_id, "analyzing_reply_failed", &e.to_string());
            None
        }
    };
    let mut tried: HashSet<String> = HashSet::new();
    loop {
        // Lock only around pool selection/snapshot; released before the network fetch
        // so other users' YouTube requests aren't serialized behind this one.
        let (cookie, snapshot) = {
            let mut pool = cookie_pool.lock().await;
            let cookie = match pool.next_cookie() {
                Some(c) => c,
                None => {
                    let status = pool.status();
                    log_trace(
                        trace_id,
                        "cookie_none",
                        &format!(
                            "status selectable={} cooldown={}",
                            status.selectable_cookies, status.cooldown_cookies
                        ),
                    );
                    let _ = send_text(api, chat_id, &format_no_cookie_available(&status)).await;
                    anyhow::bail!("no cookie available");
                }
            };
            if tried.contains(&cookie.id) {
                let status = pool.status();
                log_trace(
                    trace_id,
                    "cookie_retry_exhausted",
                    &format!(
                        "tried={tried:?} selectable={} cooldown={}",
                        status.selectable_cookies, status.cooldown_cookies
                    ),
                );
                let _ = send_text(api, chat_id, &format_no_cookie_available(&status)).await;
                anyhow::bail!("retry exhausted, no suitable cookie");
            }
            tried.insert(cookie.id.clone());
            log_trace(
                trace_id,
                "cookie_selected",
                &format!("cookie_id={} profile={}", cookie.id, cookie.profile_name),
            );
            let snapshot = pool.snapshot();
            (cookie, snapshot)
        };

        save_snapshot(database, &snapshot).await;

        match fetch_video_info(trace_id, url, &cookie.yt_dlp_browser_spec).await {
            Ok(info) => {
                log_trace(
                    trace_id,
                    "fetch_ok",
                    &format!(
                        "title={:?} heights={:?} thumbnail={}",
                        info.title,
                        info.available_heights,
                        info.thumbnail.is_some()
                    ),
                );
                if let Some(amid) = analyzing_msg_id {
                    let del = DeleteMessageParams::builder()
                        .chat_id(chat_id)
                        .message_id(amid)
                        .build();
                    if let Err(e) = api.delete_message(&del).await {
                        log_trace(trace_id, "analyzing_delete_failed", &e.to_string());
                    }
                }
                let caption = build_caption(&info);
                let photo = info
                    .thumbnail
                    .clone()
                    .unwrap_or_else(|| info.webpage_url.clone());
                let params = SendPhotoParams::builder()
                    .chat_id(chat_id)
                    .photo(FileUpload::String(photo))
                    .caption(&caption)
                    .parse_mode(ParseMode::MarkdownV2)
                    .build();
                if let Err(error) = api.send_photo(&params).await {
                    eprintln!("send_photo failed: {error}");
                    log_trace(trace_id, "send_photo_failed", &error.to_string());
                    // Fallback to text message with caption so user flow isn't interrupted
                    let fallback_params = SendMessageParams::builder()
                        .chat_id(chat_id)
                        .text(&caption)
                        .parse_mode(ParseMode::MarkdownV2)
                        .build();
                    if let Err(err2) = api.send_message(&fallback_params).await {
                        eprintln!("fallback send_message failed: {err2}");
                        let _ = send_text(
                            api,
                            chat_id,
                            &tf(
                                "youtube.send_photo_failed",
                                &[("error", &error.to_string())],
                            ),
                        )
                        .await;
                        anyhow::bail!("fallback send_message failed: {err2}");
                    }
                }
                log_trace(trace_id, "send_photo_ok", "preview photo sent");
                if let Some(desc) = info.description.as_deref() {
                    let link_preview = LinkPreviewOptions::builder().is_disabled(true).build();
                    let chunks = build_description_blockquotes(desc);
                    log_trace(
                        trace_id,
                        "description_chunks",
                        &format!("count={}", chunks.len()),
                    );
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
                let prompt_res = send_quality_prompt(
                    trace_id,
                    api,
                    chat_id,
                    user_id,
                    &cookie.yt_dlp_browser_spec,
                    &info,
                )
                .await
                .map_err(|e| e.to_string());
                if let Err(error) = prompt_res {
                    eprintln!("send quality prompt failed: {error}");
                    log_trace(trace_id, "quality_prompt_failed", &error);
                    let _ = send_text(
                        api,
                        chat_id,
                        &tf("youtube.quality.send_failed", &[("error", &error)]),
                    )
                    .await;
                    anyhow::bail!("quality prompt failed: {error}");
                }
                return Ok(());
            }
            Err(FetchError::RateLimited) => {
                crate::stats::record_error_global("youtube", "rate_limited").await;
                let (source, snapshot) = {
                    let mut pool = cookie_pool.lock().await;
                    let source = pool.mark_last_rate_limited();
                    let snapshot = if source.is_some() {
                        Some(pool.snapshot())
                    } else {
                        None
                    };
                    (source, snapshot)
                };
                if let Some(snapshot) = snapshot {
                    save_snapshot(database, &snapshot).await;
                }
                if let Some(source) = source {
                    let p = source.profile_name.clone();
                    println!(
                        "[cookie_refresh profile={p} event=cooldown_refresh_scheduled] cookie_id={} waiting 30min then refresh",
                        source.id
                    );
                    let _ = rate_limit_tx.send(source);
                }
                crate::stats::record_event_global("cookie", "429", "rate_limit", 0).await;
                eprintln!("yt-dlp 429 with cookie {}; retrying", cookie.id);
                log_trace(
                    trace_id,
                    "fetch_rate_limited",
                    &format!("cookie_id={}", cookie.id),
                );
                continue;
            }
            Err(FetchError::BadCookie(msg)) => {
                crate::stats::record_error_global("youtube", &format!("bad_cookie: {msg}")).await;
                eprintln!("bad cookie {}: {msg}; trying next", cookie.id);
                log_trace(
                    trace_id,
                    "fetch_bad_cookie",
                    &format!("cookie_id={} error={msg}", cookie.id),
                );
                continue;
            }
            Err(FetchError::Other(msg)) => {
                crate::stats::record_error_global("youtube", &format!("yt_dlp_failed: {msg}"))
                    .await;
                eprintln!("yt-dlp failed for {url}: {msg}");
                log_trace(trace_id, "fetch_failed", &msg);
                let _ = send_text(
                    api,
                    chat_id,
                    &tf("youtube.fetch_failed", &[("error", &msg)]),
                )
                .await;
                anyhow::bail!("yt-dlp failed: {msg}");
            }
        }
    }
}
