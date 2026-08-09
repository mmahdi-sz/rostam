use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{CallbackQuery, InlineKeyboardMarkup, MaybeInaccessibleMessage, ReplyMarkup},
};

use super::prices::RankPricesConfig;
use super::types::Rank;
use crate::bot::CB_USER_PANEL;
use crate::emoji::panel::{btn_icon, btn_icon_success, btn_icon_url_success};
use crate::i18n::{apply_premium_to_html, t, tf};

pub const CB_RANK_SHOP: &str = "rank:shop";
pub const CB_RANK_SELECT_PREFIX: &str = "rank:select:";

fn fmt_number(num: u64) -> String {
    let s = num.to_string();
    let mut result = String::new();
    let mut count = 0;
    for ch in s.chars().rev() {
        if count > 0 && count % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
        count += 1;
    }
    result.chars().rev().collect()
}

fn url_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 3);
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    encoded
}

pub fn build_shop_keyboard(selected: Rank, prices_cfg: &RankPricesConfig) -> InlineKeyboardMarkup {
    let make_btn = |rank: Rank, label_key: &str, icon_key: &str| {
        let title = t(label_key);
        let cb_data = format!("{CB_RANK_SELECT_PREFIX}{}", rank.as_str());
        if rank == selected {
            btn_icon_success(&title, &cb_data, icon_key)
        } else {
            btn_icon(&title, &cb_data, icon_key)
        }
    };

    let esfandyar_cb = format!("{CB_RANK_SELECT_PREFIX}esfandyar");
    let esfandyar_btn = if selected == Rank::Esfandyar {
        btn_icon_success(&t("rank.esfandyar"), &esfandyar_cb, "trophy")
    } else {
        btn_icon(&t("rank.esfandyar"), &esfandyar_cb, "trophy")
    };

    let rank_price = prices_cfg.ranks.get(selected.as_str());
    let custom_url = rank_price
        .and_then(|p| p.buy_url.as_ref())
        .filter(|u| !u.trim().is_empty());
    let buy_url = match custom_url {
        Some(url) => {
            if url.contains("{rank}") {
                url.replace("{rank}", &selected.display_name())
            } else {
                url.to_string()
            }
        }
        None => {
            let admin_clean = prices_cfg.admin_username.trim_start_matches('@');
            let base_url =
                if admin_clean.starts_with("http://") || admin_clean.starts_with("https://") {
                    admin_clean.to_string()
                } else if admin_clean.starts_with("t.me/") {
                    format!("https://{admin_clean}")
                } else {
                    format!("https://t.me/{admin_clean}")
                };

            let prefilled = tf(
                "rank.buy_prefilled_text",
                &[("rank", &selected.display_name())],
            );
            let encoded_text = url_encode(&prefilled);

            if base_url.contains('?') {
                format!("{base_url}&text={encoded_text}")
            } else {
                format!("{base_url}?text={encoded_text}")
            }
        }
    };
    let buy_label = t("rank.buy_rank_from_admin");

    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                make_btn(Rank::Dalavar, "rank.dalavar", "shield"),
                make_btn(Rank::Sohrab, "rank.sohrab", "brain"),
            ],
            vec![
                make_btn(Rank::Sepahbod, "rank.sepahbod", "medal"),
                make_btn(Rank::Rostam, "rank.rostam", "crown"),
            ],
            vec![esfandyar_btn],
            vec![btn_icon_url_success(&buy_label, &buy_url, "cart")],
            vec![btn_icon(&t("start.back"), CB_USER_PANEL, "back")],
        ])
        .build()
}

pub async fn send_rank_detail(api: &Bot, chat_id: i64, message_id: Option<i32>, rank: Rank) {
    crate::stats::record_event_global("paywall", "detail", rank.as_str(), 0).await;
    let prices_cfg = crate::rank::prices::get();
    let rank_name = rank.display_name();

    let features_key = format!("rank.features.{}", rank.as_str());
    let features_text = t(&features_key);

    let text = if rank == Rank::Dalavar {
        let free_text = tf("rank.detail_free_rank", &[("rank", &rank_name)]);
        format!("{free_text}\n\n<blockquote expandable>{features_text}</blockquote>")
    } else {
        let price_info = prices_cfg.ranks.get(rank.as_str());
        let orig_str = price_info
            .map(|p| fmt_number(p.original_toman))
            .unwrap_or_else(|| "0".into());
        let final_str = price_info
            .map(|p| fmt_number(p.final_toman))
            .unwrap_or_else(|| "0".into());
        let pct_str = price_info
            .map(|p| p.discount_pct.to_string())
            .unwrap_or_else(|| "0".into());

        let line_header = tf("rank.detail_header", &[("rank", &rank_name)]);
        let line_orig = tf("rank.detail_price_original", &[("price", &orig_str)]);
        let line_disc = tf("rank.detail_price_discount", &[("pct", &pct_str)]);
        let line_final = tf("rank.detail_price_final", &[("price", &final_str)]);
        let line_note = t("rank.detail_activate_note");

        format!(
            "{line_header}\n\n{line_orig}\n{line_disc}\n{line_final}\n\n<blockquote expandable>{features_text}</blockquote>\n\n{line_note}"
        )
    };

    let text_html = apply_premium_to_html(&text);
    let kb = build_shop_keyboard(rank, &prices_cfg);

    if let Some(msg_id) = message_id {
        let params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(msg_id)
            .text(&text_html)
            .parse_mode(ParseMode::Html)
            .reply_markup(kb.clone())
            .build();
        if api.edit_message_text(&params).await.is_ok() {
            return;
        }
    }

    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text_html)
        .parse_mode(ParseMode::Html)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(kb))
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[rank event=detail_send_failed] chat_id={chat_id} err={e}");
    }
}

pub async fn send_rank_shop(api: &Bot, chat_id: i64, message_id: Option<i32>) {
    // Defaults to selecting and showing Esfandyar rank.
    send_rank_detail(api, chat_id, message_id, Rank::Esfandyar).await;
}

pub async fn handle_rank_menu_callback(api: &Bot, cq: &CallbackQuery) {
    let cb = cq.data.as_deref().unwrap_or("");
    let msg_info = match cq.message.as_ref() {
        Some(MaybeInaccessibleMessage::Message(m)) => Some((m.chat.id, m.message_id)),
        _ => None,
    };
    let Some((chat_id, msg_id)) = msg_info else {
        return;
    };

    if cb == CB_RANK_SHOP || cb == crate::rank::paywall::CB_RANK_SHOW_MENU {
        send_rank_shop(api, chat_id, Some(msg_id)).await;
    } else if let Some(rank_str) = cb.strip_prefix(CB_RANK_SELECT_PREFIX) {
        if let Some(rank) = Rank::from_str(rank_str) {
            send_rank_detail(api, chat_id, Some(msg_id), rank).await;
        }
    }
}

pub async fn send_rank_menu(api: &Bot, chat_id: i64) {
    send_rank_shop(api, chat_id, None).await;
}
