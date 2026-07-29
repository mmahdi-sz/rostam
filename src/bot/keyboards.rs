use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{DeleteMessageParams, EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, ReplyKeyboardRemove, ReplyMarkup},
};
use rand::Rng;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::constants::*;
use crate::emoji::panel::{btn_icon, btn_icon_danger, btn_icon_plain, btn_icon_success};
use crate::i18n::{apply_premium_to_md, t};

pub async fn send_lang_picker(api: &Bot, chat_id: i64) -> crate::error::Result<()> {
    let keyboard = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon_plain("🇮🇷 پارسی | Parsi", "lang:set:fa", "")],
            vec![btn_icon_plain("🇬🇧 English", "lang:set:en", "")],
            vec![btn_icon_plain("🇮🇹 Italiano | Italian", "lang:set:it", "")],
        ])
        .build();
    api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text("زبان خود را انتخاب کنید\nChoose your language\nScegli la tua lingua")
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard))
            .build(),
    )
    .await?;
    Ok(())
}

pub async fn send_start_menu(api: &Bot, chat_id: i64) -> crate::error::Result<()> {
    let is_admin = crate::config::admin_user_id()
        .map(|id| id == chat_id)
        .unwrap_or(false);
    let remove_params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text("\u{200B}")
        .reply_markup(ReplyMarkup::ReplyKeyboardRemove(
            ReplyKeyboardRemove::builder().remove_keyboard(true).build(),
        ))
        .build();
    if let Ok(res) = api.send_message(&remove_params).await {
        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(res.result.message_id)
                    .build(),
            )
            .await;
    }
    let text = apply_premium_to_md(&t("start.welcome"));
    if let Err(e) = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(start_menu_keyboard(
                    is_admin,
                )))
                .build(),
        )
        .await
    {
        eprintln!("[bot event=send_start_menu_failed chat_id={chat_id} err={e:?}]");
        return Err(e.into());
    }
    Ok(())
}

static LAST_AI_ICON_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);

pub fn start_menu_keyboard(is_admin: bool) -> InlineKeyboardMarkup {
    const AI_ICONS: &[&str] = &[
        "gemini_logo",
        "chatgpt_logo",
        "claude_logo",
        "animated_bot_emoji",
    ];
    let last = LAST_AI_ICON_IDX.load(Ordering::Relaxed);
    let idx = {
        let mut rng = rand::thread_rng();
        let mut i = rng.gen_range(0..AI_ICONS.len());
        if i == last && AI_ICONS.len() > 1 {
            i = (i + 1) % AI_ICONS.len();
        }
        i
    };
    LAST_AI_ICON_IDX.store(idx, Ordering::Relaxed);
    let icon = AI_ICONS[idx];
    let mut rows = vec![
        vec![btn_icon_success(
            &t("start.ai_lab_button"),
            CB_START_AI_LAB,
            icon,
        )],
        vec![btn_icon_danger(
            &t("start.youtube_button"),
            CB_START_YOUTUBE,
            "clapper",
        )],
        vec![btn_icon(&t("start.tools_button"), CB_START_TOOLS, "")],
        vec![btn_icon_success(
            &t("start.panel_button"),
            CB_USER_PANEL,
            "user",
        )],
    ];
    if is_admin {
        rows.push(vec![btn_icon(
            &t("start.admin_button"),
            CB_ADMIN_PANEL,
            "stats",
        )]);
    }
    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

pub fn back_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(
            &t("start.back"),
            CB_START_PANEL,
            "back",
        )]])
        .build()
}

pub fn ai_lab_back_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(
            &t("start.back"),
            CB_START_AI_LAB,
            "back",
        )]])
        .build()
}

pub fn ai_lab_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon_success(
                &t("start.ai_denoise_button"),
                CB_AI_DENOISE,
                "soundwave",
            )],
            vec![btn_icon_success(
                &t("start.ai_upscale_button"),
                CB_AI_UPSCALE,
                "sparkles",
            )],
            vec![btn_icon_success(
                &t("start.ai_stt_button"),
                CB_AI_STT,
                "microphone",
            )],
            vec![btn_icon_success(
                &t("start.ai_sep_button"),
                CB_AI_SEP,
                "headphones",
            )],
            vec![btn_icon_success(
                &t("start.ai_gwm_button"),
                CB_AI_GWM,
                "gemini_logo",
            )],
            vec![btn_icon(&t("start.back"), CB_START_PANEL, "back")],
        ])
        .build()
}

pub async fn edit_to_ai_lab(api: &Bot, chat_id: i64, message_id: i32) -> crate::error::Result<()> {
    let text = apply_premium_to_md(&t("start.ai_lab_title"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ai_lab_keyboard())
        .build();
    api.edit_message_text(&params).await?;
    Ok(())
}

pub fn tools_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon(
                &t("tools.pdf_compress_button"),
                crate::pdfcompress::CB_TOOLS_PDF_COMPRESS,
                "",
            )],
            vec![btn_icon(
                &t("tools.ip_lookup_button"),
                crate::ip_lookup::CB_TOOLS_IP_LOOKUP,
                "",
            )],
            vec![btn_icon(
                &t("tools.surge_button"),
                crate::surge_dl::CB_TOOLS_SURGE,
                "",
            )],
            vec![btn_icon(&t("start.back"), CB_START_PANEL, "back")],
        ])
        .build()
}

pub async fn edit_to_tools(api: &Bot, chat_id: i64, message_id: i32) -> crate::error::Result<()> {
    let text = apply_premium_to_md(&t("start.tools_title"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(tools_keyboard())
        .build();
    api.edit_message_text(&params).await?;
    Ok(())
}

pub async fn edit_to_start_menu(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
) -> crate::error::Result<()> {
    let is_admin = crate::config::admin_user_id()
        .map(|id| id == chat_id)
        .unwrap_or(false);
    let text = apply_premium_to_md(&t("start.welcome"));
    if let Err(e) = api
        .edit_message_text(
            &EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(message_id)
                .text(&text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(start_menu_keyboard(is_admin))
                .build(),
        )
        .await
    {
        eprintln!("[bot event=edit_to_start_menu_failed chat_id={chat_id} err={e:?}]");
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_menu_keyboard_non_admin() {
        let kbd = start_menu_keyboard(false);
        assert!(!kbd.inline_keyboard.is_empty());
    }

    #[test]
    fn test_start_menu_keyboard_admin() {
        let kbd = start_menu_keyboard(true);
        assert!(!kbd.inline_keyboard.is_empty());
    }

    #[test]
    fn test_tools_keyboard_structure() {
        let kbd = tools_keyboard();
        assert!(!kbd.inline_keyboard.is_empty());
    }

    #[test]
    fn test_ai_lab_keyboard() {
        let kbd = ai_lab_keyboard();
        assert!(!kbd.inline_keyboard.is_empty());
    }
}
