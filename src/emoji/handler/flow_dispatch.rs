use frankenstein::{client_reqwest::Bot, types::Message};

use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState};

use super::{flow_emojis, flow_pack_choice, flow_import, flow_misc};

pub async fn handle_emoji_flow_message(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) -> bool {
    let chat_id = message.chat.id;
    let Some(db) = database else { return false };
    let client = db.client();
    let state = flow_manager.get(user_id);

    match state {
        FlowState::Idle => false,
        FlowState::AwaitingEmojis { collected } =>
            flow_emojis::handle(api, message, chat_id, user_id, flow_manager, client, collected).await,
        FlowState::AwaitingPackChoice { collected } =>
            flow_pack_choice::handle(api, message, chat_id, user_id, flow_manager, client, collected).await,
        FlowState::AwaitingPackAlias { pack_id } =>
            flow_misc::handle_pack_alias(api, message, chat_id, user_id, flow_manager, client, pack_id).await,
        FlowState::AwaitingImportFile =>
            flow_import::handle_import_file(api, message, chat_id, user_id, flow_manager, client).await,
        FlowState::AwaitingImportMode { sql } =>
            flow_import::handle_import_mode(api, message, chat_id, user_id, flow_manager, &sql).await,
        FlowState::AwaitingTestText =>
            flow_misc::handle_test_text(api, message, chat_id, user_id, flow_manager).await,
    }
}
