use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{GetCustomEmojiStickersParams, SendMessageParams},
    types::{Message, ReplyMarkup},
};

use crate::i18n::{t, tf};
use crate::emoji::{FlowManager, FlowState, PendingEmoji, panel as emoji_panel, store as emoji_store};

use super::{
    addemoji::extract_19digit_ids,
    extract::extract_custom_emojis,
    helpers::{filter_duplicates, send_all_duplicate_message, send_with_ents},
    pack_ops::send_cancel_and_panel,
};

pub(super) async fn handle(
    api: &Bot, message: &Message, chat_id: i64, user_id: i64,
    flow_manager: &mut FlowManager, client: &tokio_postgres::Client,
    trace_id: u64, mut collected: Vec<PendingEmoji>,
) -> bool {
    let msg_text = message.text.as_deref().unwrap_or("").trim();
    eprintln!(
        "[emoji_msg trace={trace_id} event=emojis_input] user_id={user_id} \
         collected_before={} has_entities={} text_preview={:?}",
        collected.len(),
        message.entities.as_ref().map(|e| e.len()).unwrap_or(0),
        crate::emoji::cache::preview(msg_text, 60),
    );

    if msg_text == t("emoji.cancel_button") {
        eprintln!("[emoji_msg trace={trace_id} event=emojis_cancel]");
        flow_manager.clear(user_id);
        send_cancel_and_panel(api, chat_id, trace_id).await;
        return true;
    }

    // 19-digit IDs
    let id_hits = extract_19digit_ids(msg_text);
    if !id_hits.is_empty() {
        eprintln!("[emoji_msg trace={trace_id} event=emojis_19digit] ids={}", id_hits.len());
        let stickers = api.get_custom_emoji_stickers(
            &GetCustomEmojiStickersParams::builder().custom_emoji_ids(id_hits).build(),
        ).await.map(|r| r.result).unwrap_or_default();
        let mut from_ids: Vec<PendingEmoji> = stickers.into_iter()
            .filter_map(|s| Some(PendingEmoji { custom_emoji_id: s.custom_emoji_id?, fallback: s.emoji.unwrap_or_else(|| "?".to_string()) }))
            .collect();
        let incoming = from_ids.len();
        let duplicates = filter_duplicates(client, user_id, &mut from_ids, &collected).await;
        eprintln!(
            "[emoji_msg trace={trace_id} event=emojis_19digit_dedup] \
             incoming={incoming} after_dedup={} dup_count={}",
            from_ids.len(), duplicates.len()
        );
        if incoming > 0 && from_ids.is_empty() && collected.is_empty() {
            eprintln!("[emoji_msg trace={trace_id} event=emojis_all_dup]");
            let _ = send_all_duplicate_message(api, chat_id, &duplicates).await;
            flow_manager.set(user_id, FlowState::AwaitingEmojis { collected });
            return true;
        }
        let added = from_ids.len();
        collected.extend(from_ids);
        let total_pages = emoji_panel::pending_total_pages(collected.len());
        let text = emoji_panel::format_pending_emojis(&collected, &duplicates, 0);
        let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
        eprintln!(
            "[emoji_msg trace={trace_id} event=emojis_pending] \
             added={added} total_collected={} packs={} pages={total_pages}",
            collected.len(), packs.len()
        );
        let r = api.send_message(
            &SendMessageParams::builder().chat_id(chat_id).text(text).parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages))).build(),
        ).await;
        eprintln!("[emoji_msg trace={trace_id} event=pending_sent] ok={}", r.is_ok());
        if let Err(e) = r { eprintln!("[emoji_msg trace={trace_id} event=pending_send_failed] err={e}"); }
        flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
        eprintln!("[emoji_msg trace={trace_id} event=state_transition] new_state=AwaitingPackChoice");
        return true;
    }

    // Custom emoji entities
    let mut new_emojis = extract_custom_emojis(message);
    let from_entities = new_emojis.len();
    eprintln!(
        "[emoji_msg trace={trace_id} event=emojis_entities] \
         from_entities={from_entities} collected_before={}",
        collected.len()
    );
    if new_emojis.is_empty() && collected.is_empty() {
        eprintln!("[emoji_msg trace={trace_id} event=emojis_nothing_found]");
        let _ = crate::bot::send_text(api, chat_id, &t("emoji.no_emoji_found")).await;
        return true;
    }
    let incoming_count = new_emojis.len();
    let duplicates = filter_duplicates(client, user_id, &mut new_emojis, &collected).await;
    eprintln!(
        "[emoji_msg trace={trace_id} event=emojis_dedup] \
         incoming={incoming_count} after_dedup={} dup_count={}",
        new_emojis.len(), duplicates.len()
    );
    if incoming_count > 0 && new_emojis.is_empty() && collected.is_empty() {
        eprintln!("[emoji_msg trace={trace_id} event=emojis_all_dup]");
        let _ = send_all_duplicate_message(api, chat_id, &duplicates).await;
        flow_manager.set(user_id, FlowState::AwaitingEmojis { collected });
        return true;
    }
    let added = new_emojis.len();
    collected.append(&mut new_emojis);

    let total_pages = emoji_panel::pending_total_pages(collected.len());
    let text = emoji_panel::format_pending_emojis(&collected, &duplicates, 0);
    let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
    eprintln!(
        "[emoji_msg trace={trace_id} event=emojis_pending] \
         added={added} total_collected={} packs={} pages={total_pages}",
        collected.len(), packs.len()
    );
    let r = api.send_message(
        &SendMessageParams::builder().chat_id(chat_id).text(text).parse_mode(ParseMode::MarkdownV2)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages))).build(),
    ).await;
    eprintln!("[emoji_msg trace={trace_id} event=pending_sent] ok={}", r.is_ok());
    if let Err(e) = r { eprintln!("[emoji_msg trace={trace_id} event=pending_send_failed] err={e}"); }
    flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
    eprintln!("[emoji_msg trace={trace_id} event=state_transition] new_state=AwaitingPackChoice");
    true
}
