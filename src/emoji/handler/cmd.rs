use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::{Message, ReplyMarkup},
};

use crate::bot::send_text;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, panel as emoji_panel, store as emoji_store, cache};
use crate::i18n::{entities_for_text, t, tf};

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
    let trace_id = cache::next_trace_id();
    eprintln!("[emoji_cmd trace={trace_id} event=emoji_cmd] user_id={user_id} chat_id={chat_id}");
    flow_manager.clear(user_id);
    if database.is_none() {
        eprintln!("[emoji_cmd trace={trace_id} event=no_db]");
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
    let r = api.send_message(&params).await;
    eprintln!("[emoji_cmd trace={trace_id} event=panel_sent] ok={}", r.is_ok());
    if let Err(e) = r { eprintln!("[emoji_cmd trace={trace_id} event=panel_send_failed] err={e}"); }
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
    let trace_id = cache::next_trace_id();
    let mut parts = rest.split_whitespace();
    let (Some(selector), Some(alias)) = (parts.next(), parts.next()) else {
        eprintln!("[emoji_cmd trace={trace_id} event=se_usage_error] user_id={user_id}");
        let _ = send_text(api, chat_id, &t("emoji.se_usage")).await;
        return;
    };
    eprintln!(
        "[emoji_cmd trace={trace_id} event=se_cmd] user_id={user_id} \
         selector={selector:?} alias={alias:?}"
    );
    let Some(db) = database else {
        eprintln!("[emoji_cmd trace={trace_id} event=no_db]");
        let _ = send_text(api, chat_id, &t("emoji.db_required")).await;
        return;
    };
    let client = db.client();
    let alias_value = if alias == "-" { None } else { Some(alias) };
    match emoji_store::set_item_alias(client, user_id, selector, alias_value).await {
        Ok(true) => {
            eprintln!("[emoji_cmd trace={trace_id} event=se_done] selector={selector:?} alias={alias_value:?}");
            let _ = send_text(api, chat_id, &tf("emoji.se_done", &[("alias", alias)])).await;
        }
        Ok(false) => {
            eprintln!("[emoji_cmd trace={trace_id} event=se_not_found] selector={selector:?}");
            let _ = send_text(api, chat_id, &t("emoji.se_not_found")).await;
        }
        Err(e) => {
            eprintln!("[emoji_cmd trace={trace_id} event=se_db_failed] selector={selector:?} err={e:?}");
        }
    }
}
