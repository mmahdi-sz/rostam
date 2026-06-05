use frankenstein::types::InlineKeyboardMarkup;

use crate::emoji::panel::{btn_icon, btn_icon_success, btn_icon_danger};
use crate::i18n::t;

pub const CB_STT_FA_BIG: &str = "stt:fa_big";
pub const CB_STT_FA_SMALL: &str = "stt:fa_small";
pub const CB_STT_EN_BIG: &str = "stt:en_big";
pub const CB_STT_EN_SMALL: &str = "stt:en_small";
pub const CB_STT_TOGGLE_DENOISE: &str = "stt:toggle_denoise";
pub const CB_STT_BACK: &str = "stt:back";
pub const CB_STT_CANCEL: &str = "stt:cancel";
pub const CB_STT_MAIN_MENU: &str = "stt:main_menu";

/// Build the language/quality selection keyboard.
pub fn config_keyboard(denoise: bool) -> InlineKeyboardMarkup {
    let denoise_text = if denoise {
        format!("{} ✅", t("stt.denoise_label"))
    } else {
        format!("{} ❌", t("stt.denoise_label"))
    };

    let denoise_icon = if denoise { "soundwave" } else { "cancel" };
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                btn_icon_success(&t("stt.language.fa_big"), CB_STT_FA_BIG, "flag_ir"),
                btn_icon(&t("stt.language.fa_small"), CB_STT_FA_SMALL, "speed_fast"),
            ],
            vec![
                btn_icon_success(&t("stt.language.en_big"), CB_STT_EN_BIG, "flag_us"),
                btn_icon(&t("stt.language.en_small"), CB_STT_EN_SMALL, "speed_fast"),
            ],
            vec![if denoise {
                btn_icon_success(&denoise_text, CB_STT_TOGGLE_DENOISE, denoise_icon)
            } else {
                btn_icon_danger(&denoise_text, CB_STT_TOGGLE_DENOISE, denoise_icon)
            }],
            vec![
                btn_icon(&t("start.back"), CB_STT_BACK, "back"),
            ],
        ])
        .build()
}

/// Build the "ready" / cancel keyboard after the user has chosen config.
pub fn ready_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                btn_icon(&t("stt.cancel_button"), CB_STT_CANCEL, "cancel"),
                btn_icon(&t("start.main_menu"), CB_STT_MAIN_MENU, "panel"),
            ],
        ])
        .build()
}
