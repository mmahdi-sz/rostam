use frankenstein::{
    AsyncTelegramApi, client_reqwest::Bot, methods::SendMessageParams, types::InlineKeyboardMarkup,
};
use redis::aio::MultiplexedConnection;
use tokio::sync::OnceCell;

use crate::emoji::panel::btn_icon_url;
use crate::i18n::tf;
use crate::youtube::trace::log_trace;

pub static REDIS_CONN: OnceCell<MultiplexedConnection> = OnceCell::const_new();

async fn redis_conn() -> Option<MultiplexedConnection> {
    REDIS_CONN
        .get_or_try_init(|| async {
            let client = redis::Client::open(crate::config::redis_url())?;
            client.get_multiplexed_async_connection().await
        })
        .await
        .ok()
        .cloned()
}

const VLC_NOTICE_TTL_SECS: u64 = 48 * 3600; // 48 hours TTL

pub fn vlc_download_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                btn_icon_url(
                    "نسخه اندروید (Google Play)",
                    "https://play.google.com/store/apps/details?id=org.videolan.vlc",
                    "android_logo",
                ),
                btn_icon_url(
                    "نسخه iOS (App Store)",
                    "https://www.videolan.org/vlc/download-ios.html",
                    "app_store",
                ),
            ],
            vec![
                btn_icon_url(
                    "نسخه ویندوز",
                    "https://get.videolan.org/vlc/3.0.23/win32/vlc-3.0.23-win32.exe",
                    "windows_logo",
                ),
                btn_icon_url(
                    "نسخه لینوکس",
                    "https://www.videolan.org/vlc/#download",
                    "linux_logo",
                ),
            ],
        ])
        .build()
}

pub async fn maybe_send_non_h264_notice(api: &Bot, chat_id: i64, codec_name: &str, trace_id: u64) {
    let key = format!("user:notice:vlc:{chat_id}");

    if let Some(mut conn) = redis_conn().await {
        let exists: u32 = redis::cmd("EXISTS")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap_or(0);
        if exists > 0 {
            log_trace(
                trace_id,
                "vlc_notice_throttled",
                &format!("chat_id={chat_id} ttl=48h_active"),
            );
            return;
        }

        let _: Result<(), _> = redis::cmd("SET")
            .arg(&key)
            .arg("1")
            .arg("EX")
            .arg(VLC_NOTICE_TTL_SECS)
            .query_async(&mut conn)
            .await;
    }

    let notice = tf("youtube.download.non_h264_notice", &[("codec", codec_name)]);
    let _ = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(notice)
                .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(
                    vlc_download_keyboard(),
                ))
                .build(),
        )
        .await;
    log_trace(
        trace_id,
        "vlc_notice_sent",
        &format!("chat_id={chat_id} ttl=48h"),
    );
}
