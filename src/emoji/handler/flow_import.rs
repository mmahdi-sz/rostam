use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::GetFileParams,
    types::Message,
};

use crate::i18n::{t, tf};
use crate::emoji::{FlowManager, FlowState, import as emoji_import, panel as emoji_panel};

use super::{
    helpers::send_with_ents,
    pack_ops::send_cancel_and_panel,
    pending::build_import_report,
};

pub(super) async fn handle_import_file(
    api: &Bot, message: &Message, chat_id: i64, user_id: i64,
    flow_manager: &mut FlowManager, client: &tokio_postgres::Client,
) -> bool {
    let text = message.text.as_deref().unwrap_or("").trim();
    if text == t("emoji.cancel_button") {
        flow_manager.clear(user_id);
        send_cancel_and_panel(api, chat_id).await;
        return true;
    }
    let Some(doc) = message.document.as_ref() else {
        let _ = crate::bot::send_text(api, chat_id, &t("emoji.import_send_file")).await;
        return true;
    };
    let file_id = doc.file_id.clone();
    let token = match crate::config::config_value("BOT_TOKEN") {
        Some(t) => t,
        None => { flow_manager.clear(user_id); return true; }
    };
    let file_path = match api.get_file(&GetFileParams::builder().file_id(file_id).build()).await {
        Ok(r) => match r.result.file_path {
            Some(p) => p,
            None => { let _ = crate::bot::send_text(api, chat_id, &t("emoji.import_failed")).await; return true; }
        },
        Err(e) => { eprintln!("get_file failed: {e}"); let _ = crate::bot::send_text(api, chat_id, &t("emoji.import_failed")).await; return true; }
    };
    let url = format!("https://api.telegram.org/file/bot{token}/{file_path}");
    let sql = match reqwest::get(&url).await {
        Ok(resp) => match resp.text().await {
            Ok(t) => t,
            Err(e) => { eprintln!("read import body failed: {e}"); let _ = crate::bot::send_text(api, chat_id, &t("emoji.import_failed")).await; return true; }
        },
        Err(e) => { eprintln!("download import file failed: {e}"); let _ = crate::bot::send_text(api, chat_id, &t("emoji.import_failed")).await; return true; }
    };
    let parsed = emoji_import::parse_sql(&sql);
    if parsed.packs.is_empty() && parsed.items.is_empty() {
        let _ = crate::bot::send_text(api, chat_id, &t("emoji.import_empty_file")).await;
        flow_manager.clear(user_id);
        return true;
    }
    let analysis = emoji_import::analyze(&parsed, client, user_id).await;
    let report = build_import_report(&analysis);
    let keyboard = emoji_panel::import_choice_keyboard(analysis.db_empty);
    send_with_ents(api, chat_id, report, Some(::frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(keyboard))).await;
    flow_manager.set(user_id, FlowState::AwaitingImportMode { sql });
    true
}

pub(super) async fn handle_import_mode(
    api: &Bot, message: &Message, chat_id: i64, user_id: i64,
    flow_manager: &mut FlowManager, sql: &str,
) -> bool {
    let text = message.text.as_deref().unwrap_or("").trim();
    if text == t("emoji.cancel_button") {
        flow_manager.clear(user_id);
        send_cancel_and_panel(api, chat_id).await;
    }
    true
}
