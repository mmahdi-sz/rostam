use super::menu::send_rank_detail;
use super::types::Rank;
use crate::i18n::{apply_premium_to_html, t, tf};
use frankenstein::{AsyncTelegramApi, ParseMode, client_reqwest::Bot, methods::SendMessageParams};

pub const CB_RANK_SHOW_MENU: &str = "rank:menu";

/// Type 1 restriction — feature is unavailable for this rank.
/// feature: Feature name.
/// min_rank: Minimum required rank.
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

/// Type 3 restriction — quota reservation failed due to DB error (fail closed).
///
/// Cancels operation and notifies user to contact admin.
/// Sent without `parse_mode` as raw text to avoid escape conflicts across HTML/MarkdownV2 handlers.
pub async fn quota_db_error(api: &Bot, chat_id: i64, feature: &str, err: &str) {
    crate::stats::record_error_global(feature, &format!("quota_reserve_failed: {err}")).await;
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(t("rank.quota_db_error"))
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[rank event=quota_db_error_send_failed] chat_id={chat_id} err={e}");
    }
}

/// Type 2 restriction — feature exists with numeric limit (duration, size, count).
/// limit: Limit description.
/// min_rank: Minimum rank required for higher limits.
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
