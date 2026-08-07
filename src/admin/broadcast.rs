//! Admin broadcast module: sending banners (copy or forward) with optional pinning.

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{CopyMessageParams, ForwardMessageParams, PinChatMessageParams},
    types::InlineKeyboardMarkup,
};
use std::sync::Arc;
use tokio_postgres::Client;

use crate::bot::constants::*;
use crate::emoji::flow::BroadcastMode;
use crate::emoji::panel::{btn_icon, btn_icon_danger, btn_icon_success};
use crate::i18n::{t, tf};
use crate::stats::{get_broadcast_user_ids, mark_user_blocked_global, record_event_global};

pub fn broadcast_menu_keyboard(pin_enabled: bool) -> InlineKeyboardMarkup {
    let pin_btn = if pin_enabled {
        btn_icon_success(
            &t("admin.broadcast.btn_pin_on"),
            CB_BROADCAST_TOGGLE_PIN,
            "check",
        )
    } else {
        btn_icon_danger(
            &t("admin.broadcast.btn_pin_off"),
            CB_BROADCAST_TOGGLE_PIN,
            "cross",
        )
    };

    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon(
                &t("admin.broadcast.btn_mode_copy"),
                CB_BROADCAST_MODE_COPY,
                "broadcast_logo",
            )],
            vec![btn_icon(
                &t("admin.broadcast.btn_mode_forward"),
                CB_BROADCAST_MODE_FORWARD,
                "forward_logo",
            )],
            vec![pin_btn],
            vec![btn_icon(&t("admin.back"), CB_ADMIN_PANEL, "back")],
        ])
        .build()
}

pub fn broadcast_target_keyboard(active_users: i64, total_users: i64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon_success(
                &tf(
                    "admin.broadcast.btn_send_active",
                    &[("active", &active_users.to_string())],
                ),
                CB_BROADCAST_SEND_ACTIVE,
                "user",
            )],
            vec![btn_icon(
                &tf(
                    "admin.broadcast.btn_send_all",
                    &[("total", &total_users.to_string())],
                ),
                CB_BROADCAST_SEND_ALL,
                "broadcast_logo",
            )],
            vec![btn_icon(&t("admin.back"), CB_ADMIN_PANEL, "back")],
        ])
        .build()
}

pub fn format_broadcast_status(
    mode: BroadcastMode,
    processed: usize,
    total: usize,
    elapsed_secs: u64,
) -> String {
    let mode_str = match mode {
        BroadcastMode::Copy => t("admin.broadcast.mode_copy"),
        BroadcastMode::Forward => t("admin.broadcast.mode_forward"),
    };

    let percent = if total > 0 {
        (processed * 100) / total
    } else {
        100
    };
    let percent = percent.min(100);
    let filled_blocks = if total > 0 {
        (processed * 16) / total
    } else {
        16
    };
    let filled_blocks = filled_blocks.min(16);
    let empty_blocks = 16 - filled_blocks;
    let bar = format!("{}{}", "█".repeat(filled_blocks), "░".repeat(empty_blocks));

    let speed = if elapsed_secs > 0 {
        (processed as u64 * 60) / elapsed_secs
    } else {
        processed as u64 * 60
    };

    tf(
        "admin.broadcast.progress_status",
        &[
            ("mode", &mode_str),
            ("count", &total.to_string()),
            ("bar", &bar),
            ("percent", &percent.to_string()),
            ("speed", &speed.to_string()),
        ],
    )
}

pub fn format_broadcast_completed_report(
    success_count: i64,
    blocked_count: i64,
    total_count: i64,
) -> String {
    tf(
        "admin.broadcast.completed",
        &[
            ("success", &success_count.to_string()),
            ("blocked", &blocked_count.to_string()),
            ("total", &total_count.to_string()),
        ],
    )
}

pub fn spawn_broadcast_job(
    api: Bot,
    db_client: Option<Arc<Client>>,
    admin_chat_id: i64,
    mode: BroadcastMode,
    pin: bool,
    banner_chat_id: i64,
    banner_message_id: i32,
    only_active: bool,
    limit: Option<i64>,
) {
    crate::app::spawn_user_task(async move {
        let trace = crate::log::next_trace_id();
        crate::log_actor_id!("broadcast", trace, admin_chat_id, "job" => "start");

        let user_ids = if let Some(ref client) = db_client {
            get_broadcast_user_ids(client, only_active, limit)
                .await
                .unwrap_or_default()
        } else {
            vec![admin_chat_id]
        };

        let total = user_ids.len();

        // اطلاع به ادمین در یک پیام جدید شامل آمار و درصد پیشرفت
        let initial_text = format_broadcast_status(mode, 0, total, 0);
        let mut status_msg_id: Option<i32> = None;
        let send_res = api
            .send_message(
                &frankenstein::methods::SendMessageParams::builder()
                    .chat_id(admin_chat_id)
                    .text(&crate::i18n::apply_premium_to_md(&initial_text))
                    .parse_mode(ParseMode::MarkdownV2)
                    .build(),
            )
            .await;

        match send_res {
            Ok(msg) => status_msg_id = Some(msg.result.message_id),
            Err(e) => eprintln!("[broadcast] failed to send initial status message: {e:?}"),
        }

        let start_time = std::time::Instant::now();
        let mut success_count = 0i64;
        let mut _blocked_count = 0i64;
        let mut last_update_time = std::time::Instant::now();

        for (idx, &uid) in user_ids.iter().enumerate() {
            let send_res: Result<i32, frankenstein::Error> = match mode {
                BroadcastMode::Copy => {
                    let copy_params = CopyMessageParams::builder()
                        .chat_id(uid)
                        .from_chat_id(banner_chat_id)
                        .message_id(banner_message_id)
                        .build();
                    api.copy_message(&copy_params)
                        .await
                        .map(|r| r.result.message_id)
                }
                BroadcastMode::Forward => {
                    let fwd_params = ForwardMessageParams::builder()
                        .chat_id(uid)
                        .from_chat_id(banner_chat_id)
                        .message_id(banner_message_id)
                        .build();
                    api.forward_message(&fwd_params)
                        .await
                        .map(|r| r.result.message_id)
                }
            };

            match send_res {
                Ok(sent_msg_id) => {
                    success_count += 1;
                    if pin {
                        let pin_params = PinChatMessageParams::builder()
                            .chat_id(uid)
                            .message_id(sent_msg_id)
                            .disable_notification(true)
                            .build();
                        let _ = api.pin_chat_message(&pin_params).await;
                    }
                }
                Err(err) => {
                    let err_str = format!("{err:?}");
                    if err_str.contains("Forbidden")
                        || err_str.contains("blocked")
                        || err_str.contains("deactivated")
                    {
                        _blocked_count += 1;
                        mark_user_blocked_global(uid).await;
                    } else {
                        _blocked_count += 1;
                    }
                }
            }

            let processed = idx + 1;
            let now = std::time::Instant::now();
            let should_update = (processed % 40 == 0)
                || (now.duration_since(last_update_time).as_secs_f32() >= 2.5)
                || (processed == total);

            if should_update {
                if let Some(msg_id) = status_msg_id {
                    let elapsed_secs = start_time.elapsed().as_secs();
                    let progress_text =
                        format_broadcast_status(mode, processed, total, elapsed_secs);

                    if let Err(e) =
                        crate::bot::edit_text_md(&api, admin_chat_id, msg_id, &progress_text, None)
                            .await
                    {
                        eprintln!("[broadcast] failed to edit status message: {e:?}");
                    }
                    last_update_time = now;
                }
            }

            // تأخیر ۶۷ میلی‌ثانیه‌ای برای رعایت سقف ۱۵ پیام در ثانیه
            tokio::time::sleep(std::time::Duration::from_millis(67)).await;
        }

        record_event_global("broadcast", "completed", "ok", success_count).await;

        // ارسال پیام گزارش نهایی جداگانه به همراه دکمه برگشت به پنل ادمین
        let final_report =
            format_broadcast_completed_report(success_count, _blocked_count, total as i64);
        let kb = frankenstein::types::InlineKeyboardMarkup::builder()
            .inline_keyboard(vec![vec![btn_icon(
                &t("admin.back"),
                CB_ADMIN_PANEL,
                "back",
            )]])
            .build();
        let _ =
            crate::bot::send_text_md_with_keyboard(&api, admin_chat_id, &final_report, kb).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_broadcast_menu_keyboard() {
        let kbd_off = broadcast_menu_keyboard(false);
        assert_eq!(kbd_off.inline_keyboard.len(), 4);
        let kbd_on = broadcast_menu_keyboard(true);
        assert_eq!(kbd_on.inline_keyboard.len(), 4);
    }

    #[test]
    fn test_broadcast_target_keyboard() {
        let kbd = broadcast_target_keyboard(10, 50);
        assert_eq!(kbd.inline_keyboard.len(), 3);
    }

    #[test]
    fn test_format_broadcast_status() {
        let status = format_broadcast_status(BroadcastMode::Copy, 50, 100, 10);
        assert!(status.contains("50%"));
        assert!(status.contains("████████░░░░░░░░"));

        let report = format_broadcast_completed_report(98, 2, 100);
        assert!(report.contains("98"));
        assert!(report.contains("100"));
    }
}
