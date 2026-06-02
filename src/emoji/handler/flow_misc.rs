use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::Message,
};

use crate::i18n::t;
use crate::emoji::{FlowManager, store as emoji_store};
use crate::emoji::cache::{self, LookupOutcome, RenderLookup};

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
    let trace_id = cache::next_trace_id();
    let raw = message.text.as_deref().unwrap_or("");
    let text = raw.trim();

    eprintln!(
        "[emoji_test trace={trace_id} event=incoming] user_id={user_id} chat_id={chat_id} \
         raw_len={raw_len} trim_len={trim_len} preview={preview:?}",
        raw_len = raw.chars().count(),
        trim_len = text.chars().count(),
        preview = cache::preview(text, 120),
    );

    if text == t("emoji.cancel_button") {
        eprintln!("[emoji_test trace={trace_id} event=cancel]");
        flow_manager.clear(user_id);
        send_cancel_and_panel(api, chat_id).await;
        return true;
    }

    let rendered = if let Some(cache_arc) = cache::global() {
        let cache_guard = cache_arc.read().await;
        eprintln!(
            "[emoji_test trace={trace_id} event=cache_state] empty={empty} key_count={keys} entry_count={entries}",
            empty = cache_guard.is_empty(),
            keys = cache_guard.key_count(),
            entries = cache_guard.entry_count(),
        );
        let (rendered, lookups) = cache_guard.render_markdown_with_trace(text);
        log_lookups(trace_id, &lookups);
        eprintln!(
            "[emoji_test trace={trace_id} event=render_summary] {summary} rendered_len={rl} rendered_preview={rp:?}",
            summary = cache::summarise_lookups(&lookups),
            rl = rendered.chars().count(),
            rp = cache::preview(&rendered, 200),
        );
        rendered
    } else {
        eprintln!("[emoji_test trace={trace_id} event=cache_state] status=not_loaded passthrough=true");
        text.to_string()
    };

    eprintln!(
        "[emoji_test trace={trace_id} event=send_attempt] parse_mode=MarkdownV2 text_len={tl}",
        tl = rendered.chars().count(),
    );

    let send_result = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&rendered)
                .parse_mode(ParseMode::MarkdownV2)
                .build(),
        )
        .await;

    match send_result {
        Ok(_) => {
            eprintln!("[emoji_test trace={trace_id} event=send_ok]");
        }
        Err(e) => {
            eprintln!(
                "[emoji_test trace={trace_id} event=send_failed] error={e} rendered_full={rendered:?}",
            );
            // Fall back to a plain-text message so the user sees *something*
            // and knows the render produced a Telegram-rejected MarkdownV2 string.
            let fallback_text = format!("{}\n\n{}", t("emoji.test_send_failed"), rendered);
            match crate::bot::send_text(api, chat_id, &fallback_text).await {
                Ok(_) => eprintln!("[emoji_test trace={trace_id} event=fallback_sent]"),
                Err(fe) => eprintln!(
                    "[emoji_test trace={trace_id} event=fallback_failed] error={fe}",
                ),
            }
        }
    }

    true
}

fn log_lookups(trace_id: u64, lookups: &[RenderLookup]) {
    for (idx, l) in lookups.iter().enumerate() {
        match &l.outcome {
            LookupOutcome::CacheHit { custom_emoji_id, fallback, group_size } => {
                eprintln!(
                    "[emoji_test trace={trace_id} event=lookup] idx={idx} key={key:?} \
                     outcome=cache_hit group_size={group_size} fallback={fallback:?} id={id}",
                    key = l.key,
                    id = custom_emoji_id,
                );
            }
            LookupOutcome::RawId => {
                eprintln!(
                    "[emoji_test trace={trace_id} event=lookup] idx={idx} key={key:?} outcome=raw_id",
                    key = l.key,
                );
            }
            LookupOutcome::NotFound => {
                eprintln!(
                    "[emoji_test trace={trace_id} event=lookup] idx={idx} key={key:?} outcome=not_found",
                    key = l.key,
                );
            }
            LookupOutcome::UnclosedBrace => {
                eprintln!(
                    "[emoji_test trace={trace_id} event=lookup] idx={idx} outcome=unclosed_brace",
                );
            }
        }
    }
}
