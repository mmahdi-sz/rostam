use std::sync::LazyLock;

use frankenstein::{
    AsyncTelegramApi, ParseMode, client_reqwest::Bot, methods::EditMessageTextParams,
};

use super::config::CompressConfig;
use super::pipeline::start_compression_task;
use super::session::show_options_menu;
use crate::bot::{edit_to_tools, send_text_with_back};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t};
use crate::log::next_trace_id;

use crate::common::job::JobRegistry;

/// Cancel flag per user so the "Cancel" button on progress message works.
/// Kills 7z/rar process to free CPU instead of discarding output.
pub(super) static ACTIVE_FC_JOBS: LazyLock<JobRegistry<i64>> = LazyLock::new(JobRegistry::new);

pub fn cancel_fc_job(user_id: i64) -> bool {
    ACTIVE_FC_JOBS.cancel(&user_id)
}

pub const CB_TOOLS_FILECOMPRESS: &str = "tools:fc";
pub const CB_FC_PREFIX: &str = "fc:";

// ── Entry point ────────────────────────────────────────────────────────────────

pub async fn enter_filecompress(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    crate::log_actor_id!("filecompress", trace_id, user_id, "clicked" => CB_TOOLS_FILECOMPRESS);
    let config = CompressConfig::default();
    flow_manager.set(
        user_id,
        FlowState::AwaitingCompressOptions {
            config: config.clone(),
        },
    );

    show_options_menu(api, chat_id, message_id, &config).await;
}

// ── Callback Handler ───────────────────────────────────────────────────────────

/// Callback response; populated text = transient toast on user screen.
async fn fc_answer(api: &Bot, cb_id: &str, text: Option<String>) {
    let b = frankenstein::methods::AnswerCallbackQueryParams::builder().callback_query_id(cb_id);
    let _ = match text {
        Some(txt) => api.answer_callback_query(&b.text(txt).build()).await,
        None => api.answer_callback_query(&b.build()).await,
    };
}

pub async fn handle_fc_callback(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
    action: &str,
    cb_id: &str,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    crate::log_ev!("filecompress", trace_id, "callback", "action" => action, "user_id" => user_id);

    // Instant ack for all callbacks except those with custom toast/alert
    if !matches!(action, "lvl:up" | "lvl:down" | "toggle:obfuscate") {
        fc_answer(api, cb_id, None).await;
    }

    if action == "cancel" {
        flow_manager.clear(user_id);
        let _ = edit_to_tools(api, chat_id, message_id).await;
        return;
    }

    if action == "jobcancel" {
        crate::log_ev!("filecompress", trace_id, "job_cancelled", "user_id" => user_id);
        cancel_fc_job(user_id);
        flow_manager.clear(user_id);
        let params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(&apply_premium_to_md(&t("fc.cancelled")))
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        let _ = api.edit_message_text(&params).await;
        return;
    }

    if action == "done" {
        let (config, files, prompt_msg_id) = match flow_manager.get(user_id) {
            FlowState::AwaitingCompressFiles {
                config,
                files,
                prompt_msg_id,
            } => (*config, files, prompt_msg_id),
            _ => return,
        };

        flow_manager.clear(user_id);

        if files.is_empty() {
            let _ = send_text_with_back(api, chat_id, &t("fc.error.no_files")).await;
            return;
        }

        start_compression_task(
            api,
            chat_id,
            prompt_msg_id,
            user_id,
            config,
            files,
            trace_id,
            database,
            flow_manager,
        )
        .await;
        return;
    }

    if action == "noop" {
        return;
    }

    super::session::handle_options_action(
        api,
        chat_id,
        message_id,
        user_id,
        flow_manager,
        action,
        cb_id,
    )
    .await;
}

// Test-only access to real keyboards and formatting (re-exported from progress module)
#[cfg(feature = "testapi")]
pub use super::progress::{
    cancel_only_keyboard_for_test, format_clock_for_test, job_cancel_keyboard_for_test,
    options_keyboard_for_test, render_progress_for_test,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_fc_cancel_lifecycle() {
        let user_id = 999_888_010;
        let flag = ACTIVE_FC_JOBS.register(user_id);
        assert!(ACTIVE_FC_JOBS.is_active(&user_id));
        assert!(!flag.load(Ordering::SeqCst));

        // simulate cancel
        let cancelled = cancel_fc_job(user_id);
        assert!(cancelled);
        assert!(flag.load(Ordering::SeqCst));
        assert!(!ACTIVE_FC_JOBS.is_active(&user_id));

        // guard drop unregister test
        let user_id_2 = 999_888_011;
        let (flag2, _guard) = ACTIVE_FC_JOBS.register_with_guard(user_id_2);
        assert!(ACTIVE_FC_JOBS.is_active(&user_id_2));
        assert!(!flag2.load(Ordering::SeqCst));
        drop(_guard);
        assert!(!ACTIVE_FC_JOBS.is_active(&user_id_2));
    }
}
