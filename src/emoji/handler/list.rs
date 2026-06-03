use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, LinkPreviewOptions, ReplyMarkup},
};

use crate::i18n::t;
use crate::emoji::{panel as emoji_panel, store as emoji_store};

pub(super) async fn send_emoji_list(
    api: &Bot, chat_id: i64, user_id: i64,
    client: &tokio_postgres::Client, trace_id: u64,
) {
    eprintln!("[emoji trace={trace_id} event=send_list_start] chat_id={chat_id} user_id={user_id}");
    let packs = match emoji_store::list_packs(client, user_id).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[emoji trace={trace_id} event=list_packs_failed] err={e:?}");
            return;
        }
    };
    if packs.is_empty() {
        eprintln!("[emoji trace={trace_id} event=list_empty]");
        let keyboard = InlineKeyboardMarkup::builder()
            .inline_keyboard(vec![vec![emoji_panel::btn(&t("emoji.panel.back"), emoji_panel::CB_BACK)]])
            .build();
        let r = api.send_message(
            &SendMessageParams::builder().chat_id(chat_id).text(t("emoji.no_packs"))
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard)).build(),
        ).await;
        eprintln!("[emoji trace={trace_id} event=no_packs_sent] ok={}", r.is_ok());
        return;
    }
    let mut packs_with_items = Vec::new();
    let mut total_items = 0usize;
    for pack in packs {
        let items = emoji_store::list_items(client, pack.id).await.unwrap_or_default();
        total_items += items.len();
        packs_with_items.push((pack, items));
    }
    eprintln!("[emoji trace={trace_id} event=list_loaded] packs={} items={total_items}", packs_with_items.len());
    let (text, page, total_pages) = emoji_panel::build_list_page(&packs_with_items, 0);
    let keyboard = emoji_panel::list_page_keyboard(page, total_pages);
    let no_preview = LinkPreviewOptions::builder().is_disabled(true).build();
    let r = api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id).text(text).parse_mode(ParseMode::MarkdownV2)
            .link_preview_options(no_preview)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard)).build(),
    ).await;
    eprintln!(
        "[emoji trace={trace_id} event=list_sent] ok={} page={page} total_pages={total_pages}",
        r.is_ok()
    );
    if let Err(e) = r {
        eprintln!("[emoji trace={trace_id} event=list_send_failed] err={e}");
    }
}

pub(super) async fn edit_emoji_list_page(
    api: &Bot, chat_id: i64, message_id: i32,
    user_id: i64, client: &tokio_postgres::Client, page: usize, trace_id: u64,
) {
    eprintln!("[emoji trace={trace_id} event=edit_list_page] chat_id={chat_id} page={page}");
    let packs = match emoji_store::list_packs(client, user_id).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[emoji trace={trace_id} event=list_packs_failed] err={e:?}");
            return;
        }
    };
    let mut packs_with_items = Vec::new();
    for pack in packs {
        let items = emoji_store::list_items(client, pack.id).await.unwrap_or_default();
        packs_with_items.push((pack, items));
    }
    let (text, page, total_pages) = emoji_panel::build_list_page(&packs_with_items, page);
    let keyboard = emoji_panel::list_page_keyboard(page, total_pages);
    let no_preview = LinkPreviewOptions::builder().is_disabled(true).build();
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id).message_id(message_id).text(text)
        .parse_mode(ParseMode::MarkdownV2).link_preview_options(no_preview)
        .reply_markup(keyboard).build();
    match api.edit_message_text(&params).await {
        Ok(_) => eprintln!("[emoji trace={trace_id} event=edit_list_ok] page={page} total_pages={total_pages}"),
        Err(e) => eprintln!("[emoji trace={trace_id} event=edit_list_failed] err={e}"),
    }
}
