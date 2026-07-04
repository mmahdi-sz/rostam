use frankenstein::{
    AsyncTelegramApi, ParseMode, client_reqwest::Bot, methods::SendMessageParams,
    types::{InlineKeyboardMarkup, ReplyMarkup},
};
use crate::i18n::{t, apply_premium_to_html};
use crate::bot::CB_USER_PANEL;
use crate::emoji::panel::btn_icon;

pub async fn send_rank_menu(api: &Bot, chat_id: i64) {
    crate::stats::record_event_global("paywall", "menu", "ok", 0).await;
    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(&t("panel.back_button"), CB_USER_PANEL, "back")]])
        .build();
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(apply_premium_to_html(&t("rank.guide")))
        .parse_mode(ParseMode::Html)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(kb))
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[rank event=menu_send_failed] chat_id={chat_id} err={e}");
    }
}
