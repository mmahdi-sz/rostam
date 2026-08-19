use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, ReplyMarkup},
};

use crate::emoji::panel::btn_icon;
use crate::i18n::{entities_for_text, t, tf};
use crate::surge_dl::types::{
    CB_SURGE_CANCEL, CB_SURGE_CONFIRM_ORIGINAL, CB_SURGE_CONFIRM_RENAME,
};

pub(crate) fn cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(
            &t("start.back"),
            CB_SURGE_CANCEL,
            "back",
        )]])
        .build()
}

pub(crate) fn confirm_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                btn_icon(
                    &t("surge.confirm_original_button"),
                    CB_SURGE_CONFIRM_ORIGINAL,
                    "check",
                ),
                btn_icon(
                    &t("surge.confirm_rename_button"),
                    CB_SURGE_CONFIRM_RENAME,
                    "edit",
                ),
            ],
            vec![btn_icon(&t("start.back"), CB_SURGE_CANCEL, "back")],
        ])
        .build()
}

pub(crate) fn build_bar(percent: f32) -> String {
    let total = 10usize;
    let filled = ((percent / 10.0).round() as i32).clamp(0, total as i32) as usize;
    let mut s = String::new();
    for _ in 0..filled {
        s.push('●');
    }
    for _ in 0..(total - filled) {
        s.push('○');
    }
    s
}

/// Format byte traffic as formatted string.
pub(crate) fn fmt_traffic_fa(bytes: u64) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    const MB: f64 = (1u64 << 20) as f64;
    let b = bytes as f64;
    let (num, unit) = if b >= GB {
        let g = b / GB;
        if (g.round() - g).abs() < 0.05 {
            (
                format!("{:.0}", g.round()),
                crate::i18n::t("youtube.unit_gb"),
            )
        } else {
            (format!("{g:.1}"), crate::i18n::t("youtube.unit_gb"))
        }
    } else {
        (
            format!("{:.0}", (b / MB).round()),
            crate::i18n::t("youtube.unit_mb"),
        )
    };
    format!("{} {}", crate::i18n::to_fa_digits(&num), unit)
}

pub(crate) fn fmt_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.2}GB", mb / 1024.0)
    } else {
        format!("{mb:.1}MB")
    }
}

// surge already reports speed/avg_speed in MB/s (confirmed against a real
// download's total_size/time_taken) — no unit conversion needed here.
pub(crate) fn fmt_speed(mb_per_sec: f64) -> String {
    format!("{mb_per_sec:.2}MB/s")
}

pub(crate) fn fmt_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Calculate transfer speed from bytes and elapsed duration.
pub(crate) fn fmt_speed_from(bytes: u64, elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64().max(0.001);
    let mb_per_sec = bytes as f64 / (1024.0 * 1024.0) / secs;
    fmt_speed(mb_per_sec)
}

pub(crate) async fn show_sent_menu(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    bytes: u64,
    download_elapsed: Duration,
    upload_elapsed: Duration,
) {
    let _ = api
        .delete_message(
            &frankenstein::methods::DeleteMessageParams::builder()
                .chat_id(chat_id)
                .message_id(message_id)
                .build(),
        )
        .await;

    let is_admin = crate::config::admin_user_id()
        .map(|id| id == chat_id)
        .unwrap_or(false);
    // Speed calculated from total bytes and measured duration.
    let text = tf(
        "surge.sent",
        &[
            ("download_time", &fmt_elapsed(download_elapsed)),
            ("download_speed", &fmt_speed_from(bytes, download_elapsed)),
            ("upload_time", &fmt_elapsed(upload_elapsed)),
            ("upload_speed", &fmt_speed_from(bytes, upload_elapsed)),
        ],
    );
    let entities = entities_for_text(&text);
    let mut params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(
            crate::bot::start_menu_keyboard(is_admin),
        ))
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    let _ = api.send_message(&params).await;
}

pub(crate) async fn edit_status(api: &Bot, chat_id: i64, message_id: i32, text: &str) {
    let entities = entities_for_text(text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    let _ = api.edit_message_text(&params).await;
}
