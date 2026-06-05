use frankenstein::types::{InlineKeyboardMarkup, InlineKeyboardButton, ButtonStyle};

use crate::emoji::panel::btn_icon;
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

    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                btn_icon(&t("stt.language.fa_big"), CB_STT_FA_BIG, "star_yt"),
                btn_icon(&t("stt.language.fa_small"), CB_STT_FA_SMALL, "signal"),
            ],
            vec![
                btn_icon(&t("stt.language.en_big"), CB_STT_EN_BIG, "star_yt"),
                btn_icon(&t("stt.language.en_small"), CB_STT_EN_SMALL, "signal"),
            ],
            vec![InlineKeyboardButton {
                text: denoise_text,
                callback_data: Some(CB_STT_TOGGLE_DENOISE.to_string()),
                style: Some(if denoise { ButtonStyle::Primary } else { ButtonStyle::Danger }),
                icon_custom_emoji_id: None,
                url: None, login_url: None, web_app: None,
                switch_inline_query: None, switch_inline_query_current_chat: None,
                switch_inline_query_chosen_chat: None, copy_text: None,
                callback_game: None, pay: None,
            }],
            vec![
                btn_icon(&t("start.back"), CB_STT_BACK, "back"),
                btn_icon(&t("start.main_menu"), CB_STT_MAIN_MENU, "panel"),
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
