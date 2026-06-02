use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::{Message, ReplyMarkup},
};

use crate::bot::send_text;
use crate::database::postgresql::PostgresDatabase;
use crate::i18n::{entities_for_text, t, tf};
use crate::emoji::{FlowManager, store as emoji_store, panel as emoji_panel};

pub async fn handle_emoji_command(
    api: &Bot,
    message: &Message,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let chat_id = message.chat.id;
    let user_id = match message.from.as_ref() {
        Some(u) => u.id as i64,
        None => return,
    };
    flow_manager.clear(user_id);
    if database.is_none() {
        let _ = send_text(api, chat_id, &t("emoji.db_required")).await;
        return;
    }
    let panel_text = emoji_panel::main_panel_text();
    let ents = entities_for_text(&panel_text);
    let params = if ents.is_empty() {
        SendMessageParams::builder().chat_id(chat_id).text(panel_text)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard())).build()
    } else {
        SendMessageParams::builder().chat_id(chat_id).text(panel_text).entities(ents)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard())).build()
    };
    let _ = api.send_message(&params).await;
}

pub async fn handle_se_command(
    api: &Bot,
    message: &Message,
    rest: &str,
    database: &Option<PostgresDatabase>,
) {
    let chat_id = message.chat.id;
    let Some(user) = message.from.as_ref() else { return };
    let user_id = user.id as i64;
    let mut parts = rest.split_whitespace();
    let (Some(selector), Some(alias)) = (parts.next(), parts.next()) else {
        let _ = send_text(api, chat_id, &t("emoji.se_usage")).await;
        return;
    };
    let Some(db) = database else {
        let _ = send_text(api, chat_id, &t("emoji.db_required")).await;
        return;
    };
    let client = db.client();
    let alias_value = if alias == "-" { None } else { Some(alias) };
    match emoji_store::set_item_alias(client, user_id, selector, alias_value).await {
        Ok(true) => { let _ = send_text(api, chat_id, &tf("emoji.se_done", &[("alias", alias)])).await; }
        Ok(false) => { let _ = send_text(api, chat_id, &t("emoji.se_not_found")).await; }
        Err(e) => { eprintln!("set_item_alias failed: {e}"); }
    }
}
