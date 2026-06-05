use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, MessageEntity, ReplyMarkup},
};

use rand::Rng;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::emoji::cache::{self, LookupOutcome, RenderLookup};
use crate::emoji::panel::{btn_icon, btn_icon_danger, btn_icon_success};
use crate::i18n::{entities_for_text, t};

pub const CB_START_EMOJI: &str = "start:emoji";
pub const CB_START_YOUTUBE: &str = "start:youtube";
pub const CB_START_PANEL: &str = "start:panel";
pub const CB_START_AI_LAB: &str = "start:ai_lab";
pub const CB_AI_DENOISE: &str = "ai:denoise";
pub const CB_AI_UPSCALE: &str = "ai:upscale";
pub const CB_AI_STT: &str = "ai:stt";

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
                cache_ents.extend(ui_ents);
                cache_ents.sort_by_key(|e| e.offset);
                return (rendered, cache_ents, Some(trace_id));
            }
        }
    }
    let entities = entities_for_text(text);
    (text.to_string(), entities, None)
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

pub async fn send_start_menu(
    api: &Bot,
    chat_id: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let text = t("start.welcome");
    let entities = entities_for_text(&text);
    let mut params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(start_menu_keyboard()))
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    api.send_message(&params).await?;
    Ok(())
}

static LAST_AI_ICON_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);

pub fn start_menu_keyboard() -> InlineKeyboardMarkup {
    const AI_ICONS: &[&str] = &["gemini_logo", "chatgpt_logo", "claude_logo", "animated_bot_emoji"];
    let last = LAST_AI_ICON_IDX.load(Ordering::Relaxed);
    let idx = {
        let mut rng = rand::thread_rng();
        let mut i = rng.gen_range(0..AI_ICONS.len());
        if i == last && AI_ICONS.len() > 1 {
            i = (i + 1) % AI_ICONS.len();
        }
        i
    };
    LAST_AI_ICON_IDX.store(idx, Ordering::Relaxed);
    let icon = AI_ICONS[idx];
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon_success(&t("start.ai_lab_button"), CB_START_AI_LAB, icon)],
            vec![btn_icon_danger(&t("start.youtube_button"), CB_START_YOUTUBE, "clapper")],
            vec![btn_icon(&t("start.emoji_button"), CB_START_EMOJI, "panel")],
        ])
        .build()
}

pub fn ai_lab_keyboard() -> InlineKeyboardMarkup {
    use frankenstein::types::InlineKeyboardButton;
    let btn = |text: &str, cb: &str| InlineKeyboardButton {
        text: text.to_string(),
        callback_data: Some(cb.to_string()),
        style: None, icon_custom_emoji_id: None, url: None, login_url: None,
        web_app: None, switch_inline_query: None, switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None, copy_text: None, callback_game: None, pay: None,
    };
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn(&t("start.ai_denoise_button"), CB_AI_DENOISE)],
            vec![btn(&t("start.ai_upscale_button"), CB_AI_UPSCALE)],
            vec![btn(&t("start.ai_stt_button"), CB_AI_STT)],
            vec![btn_icon(&t("start.back"), CB_START_PANEL, "back")],
        ])
        .build()
}

pub async fn edit_to_ai_lab(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let text = t("start.ai_lab_title");
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .reply_markup(ai_lab_keyboard())
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    api.edit_message_text(&params).await?;
    Ok(())
}

pub async fn edit_to_start_menu(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let text = t("start.welcome");
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .reply_markup(start_menu_keyboard())
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    api.edit_message_text(&params).await?;
    Ok(())
}
