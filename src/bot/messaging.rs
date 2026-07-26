use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, MessageEntity, ReplyMarkup},
};

use crate::emoji::cache::{self, LookupOutcome, RenderLookup};
use crate::i18n::entities_for_text;

pub async fn send_text(
    api: &Bot,
    chat_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (rendered, entities, trace_id) = expand_and_entify(text, chat_id).await;
    let params = if entities.is_empty() {
        SendMessageParams::builder().chat_id(chat_id).text(&rendered).build()
    } else {
        SendMessageParams::builder().chat_id(chat_id).text(&rendered).entities(entities.clone()).build()
    };
    match api.send_message(&params).await {
        Ok(_) => {
            if let Some(tid) = trace_id {
                eprintln!(
                    "[send_text trace={tid} event=send_ok] entity_count={ec}",
                    ec = entities.len(),
                );
            }
            Ok(())
        }
        Err(e) => {
            if let Some(tid) = trace_id {
                eprintln!(
                    "[send_text trace={tid} event=send_failed] chat_id={chat_id} error={e} \
                     entity_count={ec} rendered={rendered:?}",
                    ec = entities.len(),
                );
            } else {
                eprintln!("[send_text event=send_failed] chat_id={chat_id} error={e}");
            }
            Err(Box::new(e))
        }
    }
}

#[cfg(feature = "testapi")]
tokio::task_local! {
    pub static CAPTURED_EMOJIS: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>;
}

/// Expands `{key}` templates via the emoji cache (if loaded), then collects
/// entities for both the cache expansions and the UI emoji in the remaining text.
///
/// Returns `(rendered_text, entities, optional_trace_id)`. The trace id is
/// only set when cache expansion was actually attempted, so callers can
/// correlate later `send_text` log lines with the expansion that produced them.
async fn expand_and_entify(text: &str, chat_id: i64) -> (String, Vec<MessageEntity>, Option<u64>) {
    if text.contains('{') {
        if let Some(cache_arc) = cache::global() {
            let cache_guard = cache_arc.read().await;
            if !cache_guard.is_empty() {
                let trace_id = cache::next_trace_id();
                eprintln!(
                    "[send_text trace={trace_id} event=expand_start] chat_id={chat_id} \
                     key_count={kc} entry_count={ec} text_len={tl} text_preview={tp:?}",
                    kc = cache_guard.key_count(),
                    ec = cache_guard.entry_count(),
                    tl = text.chars().count(),
                    tp = cache::preview(text, 120),
                );
                let (rendered, mut cache_ents, lookups) =
                    cache_guard.render_plain_with_trace(text);
                log_lookups(trace_id, &lookups);
                
                #[cfg(feature = "testapi")]
                let _ = CAPTURED_EMOJIS.try_with(|arc| {
                    let mut b = arc.lock().unwrap();
                    for l in &lookups {
                        if let LookupOutcome::CacheHit { custom_emoji_id, fallback, .. } = &l.outcome {
                            b.push(serde_json::json!({
                                "key": l.key,
                                "custom_emoji_id": custom_emoji_id,
                                "fallback": fallback,
                            }));
                        }
                    }
                });

                let ui_ents = entities_for_text(&rendered);
                eprintln!(
                    "[send_text trace={trace_id} event=expand_done] {summary} \
                     cache_entities={ce} ui_entities={ue} rendered_len={rl} rendered_preview={rp:?}",
                    summary = cache::summarise_lookups(&lookups),
                    ce = cache_ents.len(),
                    ue = ui_ents.len(),
                    rl = rendered.chars().count(),
                    rp = cache::preview(&rendered, 200),
                );
                // Filter out ui entities whose offset already has a cache entity
                // (fallback chars from the cache can appear in EMOJI_MAP → avoid overlapping entities)
                let ui_filtered: Vec<_> = ui_ents.into_iter()
                    .filter(|ue| !cache_ents.iter().any(|ce| ce.offset == ue.offset))
                    .collect();
                cache_ents.extend(ui_filtered);
                cache_ents.sort_by_key(|e| e.offset);
                return (rendered, cache_ents, Some(trace_id));
            }
        }
    }
    let entities = entities_for_text(text);
    (text.to_string(), entities, None)
}

#[cfg(feature = "testapi")]
pub async fn expand_and_entify_for_test(text: &str, chat_id: i64) -> (String, Vec<MessageEntity>, Option<u64>) {
    expand_and_entify(text, chat_id).await
}

fn log_lookups(trace_id: u64, lookups: &[RenderLookup]) {
    for (idx, l) in lookups.iter().enumerate() {
        match &l.outcome {
            LookupOutcome::CacheHit { custom_emoji_id, fallback, group_size } => {
                eprintln!(
                    "[send_text trace={trace_id} event=lookup] idx={idx} key={key:?} \
                     outcome=cache_hit group_size={group_size} fallback={fallback:?} id={id}",
                    key = l.key,
                    id = custom_emoji_id,
                );
            }
            LookupOutcome::RawId => {
                eprintln!(
                    "[send_text trace={trace_id} event=lookup] idx={idx} key={key:?} outcome=raw_id",
                    key = l.key,
                );
            }
            LookupOutcome::NotFound => {
                eprintln!(
                    "[send_text trace={trace_id} event=lookup] idx={idx} key={key:?} outcome=not_found",
                    key = l.key,
                );
            }
            LookupOutcome::UnclosedBrace => {
                eprintln!(
                    "[send_text trace={trace_id} event=lookup] idx={idx} outcome=unclosed_brace",
                );
            }
        }
    }
}

/// Expand `{key}` templates + collect entities, then edit an existing message.
pub async fn edit_text(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    text: &str,
    reply_markup: Option<InlineKeyboardMarkup>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (rendered, entities, _) = expand_and_entify(text, chat_id).await;
    match (entities.is_empty(), reply_markup) {
        (true, None) => {
            api.edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id).message_id(message_id).text(&rendered).build(),
            ).await?;
        }
        (true, Some(kb)) => {
            api.edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id).message_id(message_id).text(&rendered).reply_markup(kb).build(),
            ).await?;
        }
        (false, None) => {
            api.edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id).message_id(message_id).text(&rendered).entities(entities).build(),
            ).await?;
        }
        (false, Some(kb)) => {
            api.edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id).message_id(message_id).text(&rendered).entities(entities).reply_markup(kb).build(),
            ).await?;
        }
    }
    Ok(())
}

/// Used for potentially long output such as STT transcription results.
pub async fn send_long_text(
    api: &Bot,
    chat_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    const MAX: usize = 4000;
    if text.chars().count() <= MAX {
        return send_text(api, chat_id, text).await;
    }
    let mut start = 0;
    let chars: Vec<char> = text.chars().collect();
    while start < chars.len() {
        let end = (start + MAX).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        send_text(api, chat_id, &chunk).await?;
        start = end;
    }
    Ok(())
}

pub async fn send_text_md(
    api: &Bot,
    chat_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .parse_mode(ParseMode::MarkdownV2)
            .build(),
    )
    .await?;
    Ok(())
}

pub async fn send_text_md_with_keyboard(
    api: &Bot,
    chat_id: i64,
    text: &str,
    reply_markup: InlineKeyboardMarkup,
) -> Result<(), Box<dyn std::error::Error>> {
    let markup = ReplyMarkup::InlineKeyboardMarkup(reply_markup);
    api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(markup)
            .build(),
    )
    .await?;
    Ok(())
}

pub async fn send_text_with_keyboard(
    api: &Bot,
    chat_id: i64,
    text: &str,
    reply_markup: InlineKeyboardMarkup,
) -> Result<(), Box<dyn std::error::Error>> {
    let (rendered, entities, _) = expand_and_entify(text, chat_id).await;
    let markup = ReplyMarkup::InlineKeyboardMarkup(reply_markup);
    let params = if entities.is_empty() {
        SendMessageParams::builder()
            .chat_id(chat_id)
            .text(&rendered)
            .reply_markup(markup)
            .build()
    } else {
        SendMessageParams::builder()
            .chat_id(chat_id)
            .text(&rendered)
            .entities(entities)
            .reply_markup(markup)
            .build()
    };
    api.send_message(&params).await?;
    Ok(())
}

pub async fn send_text_with_back(
    api: &Bot,
    chat_id: i64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let kb = super::keyboards::back_keyboard();
    send_text_with_keyboard(api, chat_id, text, kb).await
}

