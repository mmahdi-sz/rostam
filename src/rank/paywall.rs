use frankenstein::{
    AsyncTelegramApi,
    ParseMode,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup},
};
use crate::i18n::tf;
use super::types::Rank;

pub const CB_RANK_SHOW_MENU: &str = "rank:menu";

fn show_plans_keyboard() -> InlineKeyboardMarkup {
    let btn = InlineKeyboardButton::builder()
        .text(crate::i18n::t("rank.paywall_button"))
        .callback_data(CB_RANK_SHOW_MENU)
        .build();
    InlineKeyboardMarkup::builder().inline_keyboard(vec![vec![btn]]).build()
}

/// محدودیت نوع ۱ — قابلیت برای این رتبه اصلاً در دسترس نیست
/// feature: نام فارسی قابلیت، مثلاً «تبدیل صدا به متن»
/// min_rank: حداقل رتبه لازم
pub async fn block_feature(api: &Bot, chat_id: i64, feature: &str, min_rank: Rank) {
    crate::stats::record_event_global("paywall", "feature", min_rank.as_str(), 0).await;
    let text = tf("rank.paywall_feature", &[
        ("feature", feature),
        ("min_rank", min_rank.display_name()),
    ]);
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .parse_mode(ParseMode::Html)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(show_plans_keyboard()))
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[rank event=paywall_feature_send_failed] chat_id={chat_id} err={e}");
    }
}

/// محدودیت نوع ۲ — قابلیت هست ولی با محدودیت عددی (مدت، حجم، تعداد)
/// limit: توضیح محدودیت، مثلاً «۳۰ دقیقه» یا «۵ گیگابایت روزانه»
/// min_rank: حداقل رتبه برای بیشتر
pub async fn block_limit(api: &Bot, chat_id: i64, limit: &str, min_rank: Rank) {
    crate::stats::record_event_global("paywall", "limit", min_rank.as_str(), 0).await;
    let text = tf("rank.paywall_limit", &[
        ("limit", limit),
        ("min_rank", min_rank.display_name()),
    ]);
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .parse_mode(ParseMode::Html)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(show_plans_keyboard()))
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[rank event=paywall_limit_send_failed] chat_id={chat_id} err={e}");
    }
}
