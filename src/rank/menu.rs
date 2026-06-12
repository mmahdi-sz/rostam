use frankenstein::{AsyncTelegramApi, ParseMode, client_reqwest::Bot, methods::SendMessageParams};
use crate::i18n::t;

pub async fn send_rank_menu(api: &Bot, chat_id: i64) {
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(t("rank.guide"))
        .parse_mode(ParseMode::Html)
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[rank event=menu_send_failed] chat_id={chat_id} err={e}");
    }
}
