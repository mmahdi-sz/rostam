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
use super::types::FetchError;

pub async fn handle_youtube_url(
    api: &Bot,
    chat_id: i64,
    url: &str,
    cookie_pool: &mut CookiePool,
    database: &Option<PostgresDatabase>,
) {
    let mut tried: HashSet<String> = HashSet::new();
    loop {
        let cookie = match cookie_pool.next_cookie() {
            Some(c) => c,
            None => {
                let status = cookie_pool.status();
                let _ = send_text(api, chat_id, &format_no_cookie_available(&status)).await;
                return;
            }
        };
        if tried.contains(&cookie.id) {
            let status = cookie_pool.status();
            let _ = send_text(api, chat_id, &format_no_cookie_available(&status)).await;
            return;
        }
        tried.insert(cookie.id.clone());
        save_snapshot(database, cookie_pool).await;

        match fetch_video_info(url, &cookie.yt_dlp_browser_spec).await {
            Ok(info) => {
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
                    let _ = send_text(api, chat_id, &tf("youtube.send_photo_failed", &[("error", &error.to_string())])).await;
                    return;
                }
                if let Some(desc) = info.description.as_deref() {
                    if desc.chars().count() > 1000 {
                        let link_preview = LinkPreviewOptions::builder().is_disabled(true).build();
                        for chunk in build_description_blockquotes(desc) {
                            let msg = SendMessageParams::builder()
                                .chat_id(chat_id)
                                .text(chunk)
                                .parse_mode(ParseMode::MarkdownV2)
                                .link_preview_options(link_preview.clone())
                                .build();
                            if let Err(error) = api.send_message(&msg).await {
                                eprintln!("send description chunk failed: {error}");
                                break;
                            }
                        }
                    }
                }
                if let Err(error) = send_quality_prompt(api, chat_id).await {
                    eprintln!("send quality prompt failed: {error}");
                    let _ = send_text(api, chat_id, &tf("youtube.quality.send_failed", &[("error", &error.to_string())])).await;
                }
                return;
            }
            Err(FetchError::RateLimited) => {
                if cookie_pool.mark_last_rate_limited() == Some(true) {
                    save_snapshot(database, cookie_pool).await;
                }
                eprintln!("yt-dlp 429 with cookie {}; retrying", cookie.id);
                continue;
            }
            Err(FetchError::BadCookie(msg)) => {
                eprintln!("bad cookie {}: {msg}; trying next", cookie.id);
                continue;
            }
            Err(FetchError::Other(msg)) => {
                eprintln!("yt-dlp failed for {url}: {msg}");
                let _ = send_text(api, chat_id, &tf("youtube.fetch_failed", &[("error", &msg)])).await;
                return;
            }
        }
    }
}
