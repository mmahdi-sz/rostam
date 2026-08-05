use super::menu::send_rank_detail;
use super::types::Rank;
use crate::i18n::{apply_premium_to_html, tf};
use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::SendMessageParams,
};

pub const CB_RANK_SHOW_MENU: &str = "rank:menu";

/// محدودیت نوع ۱ — قابلیت برای این رتبه اصلاً در دسترس نیست
/// feature: نام فارسی قابلیت، مثلاً «تبدیل صدا به متن»
/// min_rank: حداقل رتبه لازم
pub async fn block_feature(api: &Bot, chat_id: i64, feature: &str, min_rank: Rank) {
    crate::stats::record_event_global("paywall", "feature", min_rank.as_str(), 0).await;
    let min_rank_name = min_rank.display_name();
    let text = tf(
        "rank.paywall_feature",
        &[("feature", feature), ("min_rank", &min_rank_name)],
    );
    let text_html = apply_premium_to_html(&text);
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text_html)
        .parse_mode(ParseMode::Html)
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[rank event=paywall_feature_send_failed] chat_id={chat_id} err={e}");
    }

    send_rank_detail(api, chat_id, None, min_rank).await;
}

/// محدودیت نوع ۲ — قابلیت هست ولی با محدودیت عددی (مدت، حجم، تعداد)
/// limit: توضیح محدودیت، مثلاً «۳۰ دقیقه» یا «۵ گیگابایت روزانه»
/// min_rank: حداقل رتبه برای بیشتر
pub async fn block_limit(api: &Bot, chat_id: i64, limit: &str, min_rank: Rank) {
    crate::stats::record_event_global("paywall", "limit", min_rank.as_str(), 0).await;
    let min_rank_name = min_rank.display_name();
    let text = tf(
        "rank.paywall_limit",
        &[("limit", limit), ("min_rank", &min_rank_name)],
    );
    let text_html = apply_premium_to_html(&text);
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text_html)
        .parse_mode(ParseMode::Html)
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[rank event=paywall_limit_send_failed] chat_id={chat_id} err={e}");
    }

    send_rank_detail(api, chat_id, None, min_rank).await;
}
