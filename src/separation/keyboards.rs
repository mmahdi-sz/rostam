use frankenstein::types::InlineKeyboardMarkup;

use crate::emoji::panel::{btn_icon, btn_icon_danger, btn_icon_primary, btn_icon_success};
use crate::i18n::t;

#[allow(dead_code)]
pub const CB_AI_SEP: &str = "ai:sep";
pub const CB_SEP_PREFIX: &str = "sep:";
pub const CB_SEP_BACK: &str = "sep:back";
pub const CB_SEP_QUEUE_CANCEL: &str = "sep:qcancel";

pub fn queue_cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("separation.queue.cancel_btn"),
            CB_SEP_QUEUE_CANCEL,
            "cancel",
        )]])
        .build()
}

pub fn prompt_keyboard(msg_id: i32) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(
            &t("start.back"),
            &format!("{CB_SEP_BACK}:{msg_id}"),
            "back",
        )]])
        .build()
}

pub fn mode_keyboard(msg_id: i32) -> InlineKeyboardMarkup {
    // 1 col, 3 rows: High Quality (green) / Fast (blue) / Cancel (red).
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon_success(
                &t("separation.btn.quality"),
                &format!("sep:quality:{msg_id}"),
                "quality_high",
            )],
            vec![btn_icon_primary(
                &t("separation.btn.fast"),
                &format!("sep:fast:{msg_id}"),
                "speed_fast",
            )],
            vec![btn_icon_danger(
                &t("separation.btn.cancel"),
                &format!("sep:cancel:{msg_id}"),
                "cancel",
            )],
        ])
        .build()
}
