use frankenstein::{client_reqwest::Bot, types::Message};

use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState};

use super::{flow_emojis, flow_import, flow_misc, flow_pack_choice};

pub async fn handle_emoji_flow_message(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) -> bool {
    let trace_id = crate::log::next_trace_id();
    let chat_id = message.chat.id;
    let text_preview = message
        .text
        .as_deref()
        .map(|t| crate::emoji::cache::preview(t, 80))
        .unwrap_or_else(|| "<no_text>".to_string());
    let has_doc = message.document.is_some();

    let Some(db) = database else {
        eprintln!("[emoji_msg trace={trace_id} event=no_db] user_id={user_id} chat_id={chat_id}");
        return false;
    };
    let client = db.client();
    let state = flow_manager.get(user_id);

    eprintln!(
        "[emoji_msg trace={trace_id} event=dispatch] user_id={user_id} chat_id={chat_id} \
         state={state_name} text_preview={text_preview:?} has_doc={has_doc}",
        state_name = state_name(&state),
    );

    match state {
        FlowState::Idle => {
            eprintln!("[emoji_msg trace={trace_id} event=idle_skip]");
            false
        }
        FlowState::AwaitingEmojis { collected } => {
            eprintln!(
                "[emoji_msg trace={trace_id} event=handler_call] handler=flow_emojis collected={}",
                collected.len()
            );
            flow_emojis::handle(
                api,
                message,
                chat_id,
                user_id,
                flow_manager,
                client,
                trace_id,
                collected,
            )
            .await
        }
        FlowState::AwaitingPackChoice { collected } => {
            eprintln!(
                "[emoji_msg trace={trace_id} event=handler_call] handler=flow_pack_choice collected={}",
                collected.len()
            );
            flow_pack_choice::handle(
                api,
                message,
                chat_id,
                user_id,
                flow_manager,
                client,
                trace_id,
                collected,
            )
            .await
        }
        FlowState::AwaitingPackAlias { pack_id } => {
            eprintln!(
                "[emoji_msg trace={trace_id} event=handler_call] handler=flow_misc::pack_alias pack_id={pack_id}"
            );
            flow_misc::handle_pack_alias(
                api,
                message,
                chat_id,
                user_id,
                flow_manager,
                client,
                trace_id,
                pack_id,
            )
            .await
        }
        FlowState::AwaitingImportFile => {
            eprintln!("[emoji_msg trace={trace_id} event=handler_call] handler=flow_import::file");
            flow_import::handle_import_file(
                api,
                message,
                chat_id,
                user_id,
                flow_manager,
                client,
                trace_id,
            )
            .await
        }
        FlowState::AwaitingImportMode { sql } => {
            eprintln!("[emoji_msg trace={trace_id} event=handler_call] handler=flow_import::mode");
            flow_import::handle_import_mode(
                api,
                message,
                chat_id,
                user_id,
                flow_manager,
                trace_id,
                &sql,
            )
            .await
        }
        FlowState::AwaitingTestText => {
            eprintln!(
                "[emoji_msg trace={trace_id} event=handler_call] handler=flow_misc::test_text"
            );
            flow_misc::handle_test_text(api, message, chat_id, user_id, flow_manager).await
        }
        FlowState::AwaitingSttConfig { .. } | FlowState::AwaitingSttAudio { .. } => {
            eprintln!("[emoji_msg trace={trace_id} event=stt_skip] — handled in main");
            false
        }
        FlowState::AwaitingDenoiseAudio => {
            eprintln!("[emoji_msg trace={trace_id} event=denoise_skip] — handled in main");
            false
        }
        FlowState::AwaitingUpscaleImage { .. } => {
            eprintln!("[emoji_msg trace={trace_id} event=upscale_skip] — handled in main");
            false
        }
        FlowState::AwaitingSeparation
        | FlowState::AwaitingSeparationMode { .. }
        | FlowState::AwaitingSeparationQueued { .. } => {
            eprintln!("[emoji_msg trace={trace_id} event=separation_skip] — handled in main");
            false
        }
        FlowState::AwaitingGeminiWmImage => {
            eprintln!("[emoji_msg trace={trace_id} event=gwm_skip] — handled in main");
            false
        }
        FlowState::AwaitingPdfCompressFile | FlowState::AwaitingPdfCompressLevel { .. } => {
            eprintln!("[emoji_msg trace={trace_id} event=pdfcompress_skip] — handled in main");
            false
        }
        FlowState::AwaitingIpLookupInput => {
            eprintln!("[emoji_msg trace={trace_id} event=ip_lookup_skip] — handled in main");
            false
        }
        FlowState::AwaitingSurgeUrlInput
        | FlowState::AwaitingSurgeConfirm { .. }
        | FlowState::AwaitingSurgeRenameInput { .. } => {
            eprintln!("[emoji_msg trace={trace_id} event=surge_dl_skip] — handled in main");
            false
        }
        FlowState::AwaitingRedeemGenArgs => {
            eprintln!("[emoji_msg trace={trace_id} event=redeem_gen_skip] — handled in main");
            false
        }
        FlowState::AwaitingForceJoinLink
        | FlowState::AwaitingForceJoinPrivateInfo { .. }
        | FlowState::AwaitingForceJoinField { .. } => {
            eprintln!("[emoji_msg trace={trace_id} event=force_join_skip] — handled in main");
            false
        }
    }
}

fn state_name(state: &FlowState) -> &'static str {
    match state {
        FlowState::Idle => "Idle",
        FlowState::AwaitingEmojis { .. } => "AwaitingEmojis",
        FlowState::AwaitingPackChoice { .. } => "AwaitingPackChoice",
        FlowState::AwaitingPackAlias { .. } => "AwaitingPackAlias",
        FlowState::AwaitingImportFile => "AwaitingImportFile",
        FlowState::AwaitingImportMode { .. } => "AwaitingImportMode",
        FlowState::AwaitingTestText => "AwaitingTestText",
        FlowState::AwaitingSttConfig { .. } => "AwaitingSttConfig",
        FlowState::AwaitingSttAudio { .. } => "AwaitingSttAudio",
        FlowState::AwaitingDenoiseAudio => "AwaitingDenoiseAudio",
        FlowState::AwaitingUpscaleImage { .. } => "AwaitingUpscaleImage",
        FlowState::AwaitingSeparation => "AwaitingSeparation",
        FlowState::AwaitingSeparationMode { .. } => "AwaitingSeparationMode",
        FlowState::AwaitingSeparationQueued { .. } => "AwaitingSeparationQueued",
        FlowState::AwaitingGeminiWmImage => "AwaitingGeminiWmImage",
        FlowState::AwaitingPdfCompressFile => "AwaitingPdfCompressFile",
        FlowState::AwaitingPdfCompressLevel { .. } => "AwaitingPdfCompressLevel",
        FlowState::AwaitingIpLookupInput => "AwaitingIpLookupInput",
        FlowState::AwaitingSurgeUrlInput => "AwaitingSurgeUrlInput",
        FlowState::AwaitingSurgeConfirm { .. } => "AwaitingSurgeConfirm",
        FlowState::AwaitingSurgeRenameInput { .. } => "AwaitingSurgeRenameInput",
        FlowState::AwaitingRedeemGenArgs => "AwaitingRedeemGenArgs",
        FlowState::AwaitingForceJoinLink => "AwaitingForceJoinLink",
        FlowState::AwaitingForceJoinPrivateInfo { .. } => "AwaitingForceJoinPrivateInfo",
        FlowState::AwaitingForceJoinField { .. } => "AwaitingForceJoinField",
    }
}
