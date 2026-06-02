use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::Message,
};

use crate::i18n::t;
use crate::emoji::{FlowManager, store as emoji_store};

use super::pack_ops::send_cancel_and_panel;

pub(super) async fn handle_pack_alias(
    api: &Bot, message: &Message, chat_id: i64, user_id: i64,
    flow_manager: &mut FlowManager, client: &tokio_postgres::Client,
    pack_id: i32,
) -> bool {
    let text = message.text.as_deref().unwrap_or("").trim();
    let alias = if text == "-" || text.is_empty() { None } else { Some(text) };
    if let Err(e) = emoji_store::set_pack_alias(client, user_id, pack_id, alias).await {
        eprintln!("set_pack_alias failed: {e}");
    }
    let _ = crate::bot::send_text(api, chat_id, &t("emoji.pack_alias_set")).await;
    flow_manager.clear(user_id);
    true
}

pub(super) async fn handle_test_text(
    api: &Bot, message: &Message, chat_id: i64, user_id: i64,
    flow_manager: &mut FlowManager,
) -> bool {
    let text = message.text.as_deref().unwrap_or("").trim();
    if text == t("emoji.cancel_button") {
        flow_manager.clear(user_id);
        send_cancel_and_panel(api, chat_id).await;
        return true;
    }
    let rendered = if let Some(cache_arc) = crate::emoji::cache::global() {
        let cache = cache_arc.read().await;
        cache.render_markdown(text)
    } else {
        text.to_string()
    };
    let _ = api.send_message(
        &SendMessageParams::builder().chat_id(chat_id).text(rendered).parse_mode(ParseMode::MarkdownV2).build(),
    ).await;
    true
}
