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
    trace_id: u64,
) -> bool {
    let text = message.text.as_deref().unwrap_or("").trim();
    let has_doc = message.document.is_some();
    eprintln!(
        "[emoji_msg trace={trace_id} event=import_file_input] user_id={user_id} \
         has_doc={has_doc} text_preview={:?}",
        crate::emoji::cache::preview(text, 40)
    );

    if text == t("emoji.cancel_button") {
        eprintln!("[emoji_msg trace={trace_id} event=import_cancel]");
        flow_manager.clear(user_id);
        send_cancel_and_panel(api, chat_id, trace_id).await;
        return true;
    }
    let Some(doc) = message.document.as_ref() else {
        eprintln!("[emoji_msg trace={trace_id} event=import_no_doc]");
        let _ = crate::bot::send_text(api, chat_id, &t("emoji.import_send_file")).await;
        return true;
    };
    let file_id = doc.file_id.clone();
    eprintln!("[emoji_msg trace={trace_id} event=import_get_file] file_id={file_id}");
    let token = match crate::config::config_value("BOT_TOKEN") {
        Some(t) => t,
        None => {
            eprintln!("[emoji_msg trace={trace_id} event=import_no_token]");
            flow_manager.clear(user_id);
            return true;
        }
    };
    let file_path = match api.get_file(&GetFileParams::builder().file_id(file_id).build()).await {
        Ok(r) => match r.result.file_path {
            Some(p) => {
                eprintln!("[emoji_msg trace={trace_id} event=import_file_path] path={p:?}");
                p
            }
            None => {
                eprintln!("[emoji_msg trace={trace_id} event=import_no_path]");
                let _ = crate::bot::send_text(api, chat_id, &t("emoji.import_failed")).await;
                return true;
            }
        },
        Err(e) => {
            eprintln!("[emoji_msg trace={trace_id} event=import_get_file_failed] err={e}");
            let _ = crate::bot::send_text(api, chat_id, &t("emoji.import_failed")).await;
            return true;
        }
    };
    let url = format!("https://api.telegram.org/file/bot{token}/{file_path}");
    let sql = match reqwest::get(&url).await {
        Ok(resp) => match resp.text().await {
            Ok(t) => {
                eprintln!("[emoji_msg trace={trace_id} event=import_downloaded] bytes={}", t.len());
                t
            }
            Err(e) => {
                eprintln!("[emoji_msg trace={trace_id} event=import_read_failed] err={e}");
                let _ = crate::bot::send_text(api, chat_id, &t("emoji.import_failed")).await;
                return true;
            }
        },
        Err(e) => {
            eprintln!("[emoji_msg trace={trace_id} event=import_download_failed] err={e}");
            let _ = crate::bot::send_text(api, chat_id, &t("emoji.import_failed")).await;
            return true;
        }
    };
    let parsed = emoji_import::parse_sql(&sql);
    eprintln!(
        "[emoji_msg trace={trace_id} event=import_parsed] packs={} items={}",
        parsed.packs.len(), parsed.items.len()
    );
    if parsed.packs.is_empty() && parsed.items.is_empty() {
        eprintln!("[emoji_msg trace={trace_id} event=import_empty]");
        let _ = crate::bot::send_text(api, chat_id, &t("emoji.import_empty_file")).await;
        flow_manager.clear(user_id);
        return true;
    }
    let analysis = emoji_import::analyze(&parsed, client, user_id).await;
    eprintln!(
        "[emoji_msg trace={trace_id} event=import_analyzed] db_empty={} \
         file_packs={} file_items={} dups={}",
        analysis.db_empty, analysis.file_packs, analysis.file_items, analysis.duplicate_items
    );
    let report = build_import_report(&analysis);
    let keyboard = emoji_panel::import_choice_keyboard(analysis.db_empty);
    send_with_ents(api, chat_id, report, Some(::frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(keyboard))).await;
    flow_manager.set(user_id, FlowState::AwaitingImportMode { sql });
    eprintln!("[emoji_msg trace={trace_id} event=state_transition] new_state=AwaitingImportMode");
    true
}

pub(super) async fn handle_import_mode(
    api: &Bot, message: &Message, chat_id: i64, user_id: i64,
    flow_manager: &mut FlowManager, trace_id: u64, sql: &str,
) -> bool {
    let text = message.text.as_deref().unwrap_or("").trim();
    eprintln!(
        "[emoji_msg trace={trace_id} event=import_mode_input] user_id={user_id} \
         sql_len={} text_preview={:?}",
        sql.len(),
        crate::emoji::cache::preview(text, 40)
    );
    if text == t("emoji.cancel_button") {
        eprintln!("[emoji_msg trace={trace_id} event=import_mode_cancel]");
        flow_manager.clear(user_id);
        send_cancel_and_panel(api, chat_id, trace_id).await;
    }
    true
}
