use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup},
};

use crate::emoji::panel::btn_icon_success;
use crate::force_join::db::list_locks;
use crate::force_join::types::CB_FJ_CHECK;
use crate::force_join::ui::{no_preview, url_button};
use crate::i18n::t;

/// Lock message: all locks (mandatory + optional) rendered as link buttons.
/// Reserve link (if present) rendered in same row next to main link.
pub async fn send_lock_message(api: &Bot, chat_id: i64) {
    let locks = list_locks().await;
    let mut rows: Vec<Vec<InlineKeyboardButton>> = locks
        .iter()
        .map(|l| {
            let mut row = vec![url_button(&l.display_name(), &l.link)];
            if !l.reserve_link.is_empty() {
                row.push(url_button(
                    &t("force_join.reserve_link_label"),
                    &l.reserve_link,
                ));
            }
            row
        })
        .collect();
    rows.push(vec![btn_icon_success(
        &t("force_join.check_button"),
        CB_FJ_CHECK,
        "check",
    )]);
    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build();
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(t("force_join.locked_message"))
        .link_preview_options(no_preview())
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(kb))
        .build();
    let _ = api.send_message(&params).await;
}
