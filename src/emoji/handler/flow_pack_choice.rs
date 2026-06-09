use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::{Message, ReplyKeyboardRemove, ReplyMarkup},
};

use crate::i18n::t;
use crate::emoji::{FlowManager, FlowState, PendingEmoji, panel as emoji_panel, store as emoji_store};

use super::{
    extract::extract_custom_emojis,
    helpers::{filter_duplicates, send_all_duplicate_message, send_with_ents},
    pack_ops::send_cancel_and_panel,
    pending::apply_edit_ops,
};

pub(super) async fn handle(
    api: &Bot, message: &Message, chat_id: i64, user_id: i64,
    flow_manager: &mut FlowManager, client: &tokio_postgres::Client,
    trace_id: u64, mut collected: Vec<PendingEmoji>,
) -> bool {
    let text = message.text.as_deref().unwrap_or("").trim();
    eprintln!(
        "[emoji_msg trace={trace_id} event=pack_choice_input] user_id={user_id} \
         collected={} text_preview={:?}",
        collected.len(),
        crate::emoji::cache::preview(text, 60),
    );

    if text == t("emoji.cancel_button") {
        eprintln!("[emoji_msg trace={trace_id} event=pack_choice_cancel]");
        flow_manager.clear(user_id);
        send_cancel_and_panel(api, chat_id, trace_id).await;
        return true;
    }

    // New custom emoji entities sent while in pack-choice state
    let mut extras = extract_custom_emojis(message);
    if !extras.is_empty() {
        let incoming = extras.len();
        let duplicates = filter_duplicates(client, user_id, &mut extras, &collected).await;
        eprintln!(
            "[emoji_msg trace={trace_id} event=pack_choice_extra_emojis] \
             incoming={incoming} after_dedup={} dups={}",
            extras.len(), duplicates.len()
        );
        if incoming > 0 && extras.is_empty() {
            let _ = send_all_duplicate_message(api, chat_id, &duplicates).await;
            flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
            return true;
        }
        collected.extend(extras);
        let total_pages = emoji_panel::pending_total_pages(collected.len());
        let summary = emoji_panel::format_pending_emojis(&collected, &duplicates, 0);
        let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
        let r = api.send_message(
            &SendMessageParams::builder().chat_id(chat_id).text(summary).parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages))).build(),
        ).await;
        eprintln!("[emoji_msg trace={trace_id} event=pending_sent] ok={}", r.is_ok());
        flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
        return true;
    }

    // +N / -N filter ops
    if text.starts_with('-') || text.starts_with('+') {
        eprintln!("[emoji_msg trace={trace_id} event=pack_choice_edit_ops] ops={:?}", crate::emoji::cache::preview(text, 40));
        if apply_edit_ops(&mut collected, text).is_err() {
            let _ = crate::bot::send_text(api, chat_id, &t("emoji.pending.mixed_ops")).await;
            flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
            return true;
        }
        eprintln!("[emoji_msg trace={trace_id} event=edit_ops_applied] remaining={}", collected.len());
        let total_pages = emoji_panel::pending_total_pages(collected.len());
        let summary = emoji_panel::format_pending_emojis(&collected, &[], 0);
        let packs = emoji_store::list_packs(client, user_id).await.unwrap_or_default();
        let r = api.send_message(
            &SendMessageParams::builder().chat_id(chat_id).text(summary).parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::pack_choice_keyboard(&packs, 0, total_pages))).build(),
        ).await;
        eprintln!("[emoji_msg trace={trace_id} event=pending_sent] ok={}", r.is_ok());
        flow_manager.set(user_id, FlowState::AwaitingPackChoice { collected });
        return true;
    }

    if text.is_empty() { return true; }

    // URL sent while waiting for pack name — abort and let dispatch handle it as a link
    if text.starts_with("http") {
        eprintln!("[emoji_msg trace={trace_id} event=pack_choice_url_abort] url_preview={:?}", &text[..text.len().min(60)]);
        flow_manager.clear(user_id);
        return false;
    }

    // Pack name typed — find or create pack
    eprintln!("[emoji_msg trace={trace_id} event=pack_name_typed] name={text:?}");
    let pack = match emoji_store::find_pack_by_name(client, user_id, text).await {
        Ok(Some(p)) => {
            eprintln!("[emoji_msg trace={trace_id} event=pack_found] id={} name={:?}", p.id, p.name);
            p
        }
        Ok(None) => {
            eprintln!("[emoji_msg trace={trace_id} event=pack_create_attempt] name={text:?}");
            match emoji_store::create_pack(client, user_id, text).await {
                Ok(p) => {
                    eprintln!("[emoji_msg trace={trace_id} event=pack_created] id={} name={:?}", p.id, p.name);
                    p
                }
                Err(e) => {
                    eprintln!("[emoji_msg trace={trace_id} event=pack_create_failed] name={text:?} err={e:?}");
                    let _ = crate::bot::send_text(api, chat_id, &t("emoji.pack_create_failed")).await;
                    flow_manager.clear(user_id);
                    return true;
                }
            }
        }
        Err(e) => {
            eprintln!("[emoji_msg trace={trace_id} event=find_pack_failed] name={text:?} err={e:?}");
            let _ = crate::bot::send_text(api, chat_id, &t("emoji.pack_create_failed")).await;
            flow_manager.clear(user_id);
            return true;
        }
    };

    let mut added = 0;
    for emoji in &collected {
        let smart = match emoji_store::allocate_smart_name(client, user_id, &emoji.fallback).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[emoji_msg trace={trace_id} event=allocate_name_failed] fallback={:?} err={e:?}", emoji.fallback);
                continue;
            }
        };
        eprintln!(
            "[emoji_msg trace={trace_id} event=add_item_attempt] id={} smart_name={smart:?}",
            emoji.custom_emoji_id
        );
        match emoji_store::add_item(client, user_id, pack.id, &emoji.custom_emoji_id, &emoji.fallback, &smart).await {
            Ok(item) => {
                eprintln!("[emoji_msg trace={trace_id} event=add_item_ok] db_id={} smart_name={:?}", item.id, item.smart_name);
                added += 1;
            }
            Err(e) => {
                eprintln!("[emoji_msg trace={trace_id} event=add_item_failed] err={e:?}");
            }
        }
    }
    eprintln!("[emoji_msg trace={trace_id} event=pack_choice_done] added={added} total={}", collected.len());

    send_with_ents(api, chat_id,
        crate::i18n::tf("emoji.added_summary", &[("count", &added.to_string()), ("pack", &pack.name)]),
        Some(ReplyMarkup::ReplyKeyboardRemove(ReplyKeyboardRemove::builder().remove_keyboard(true).build()))).await;
    send_with_ents(api, chat_id, emoji_panel::main_panel_text(),
        Some(ReplyMarkup::InlineKeyboardMarkup(emoji_panel::main_panel_keyboard()))).await;
    eprintln!("[emoji_msg trace={trace_id} event=state_clear]");
    flow_manager.clear(user_id);
    true
}
