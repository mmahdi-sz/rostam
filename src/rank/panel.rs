use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{AnswerCallbackQueryParams, EditMessageTextParams, SendMessageParams},
    types::{CallbackQuery, InlineKeyboardMarkup, ReplyMarkup},
};

use crate::database::postgresql::PostgresDatabase;
use crate::i18n::{t, tf, apply_premium_to_html};
use crate::emoji::panel::{btn_icon, btn_icon_success};
use super::quota;
use super::store::get_user_rank;
use super::types::Rank;

pub const CB_USER_PANEL: &str = "user:panel";
pub const CB_USER_PANEL_MORE: &str = "user:panel:more";

// ── تبدیل epoch → شمسی (بدون crate) ─────────────────────────────────────────
// ponytail: الگوریتم Jalali خالص، فقط برای نمایش تاریخ انقضا.
fn epoch_to_jalali(epoch: i64) -> (i32, u32, u32) {
    const TEHRAN: i64 = 3 * 3600 + 30 * 60;
    let days_from_epoch = (epoch + TEHRAN) / 86_400;
    // Julian Day Number
    let jdn = days_from_epoch + 2_440_588;
    let j = jdn - 1_948_440 + 10632;
    let n = (j - 1) / 10631;
    let j = j - 10631 * n + 354;
    let j2 = ((183 + (j * 20 - 3510) / 10631) / 182) * 15;
    let year = 979 + 30 * n + (j2 * 20 - 3510) / 10631;
    let j = j - j2;
    let month = (j - 14) / 30 + 1;
    let day = j - 29 * (month - 1) - (if month <= 6 { 0 } else { month - 7 });
    (year as i32, month as u32, day as u32)
}

fn fmt_jalali(epoch: i64) -> String {
    let (y, m, d) = epoch_to_jalali(epoch);
    format!("{}/{:02}/{:02}", y, m, d)
}

fn days_left(expires_at: i64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    ((expires_at - now) / 86_400).max(0)
}

// ── نوار پیشرفت ───────────────────────────────────────────────────────────────
fn bar(used: u64, limit: u64, allowed: bool) -> String {
    if !allowed {
        return "▓▓▓▓▓".to_string();
    }
    if limit == 0 {
        return "▓▓▓▓▓".to_string();
    }
    let pct = (used * 5 / limit).min(5) as usize;
    let filled: String = "▓".repeat(pct);
    let empty: String = "░".repeat(5 - pct);
    format!("{}{}", filled, empty)
}

fn fmt_gib(bytes: u64) -> String {
    let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gib < 1.0 {
        let mib = bytes as f64 / (1024.0 * 1024.0);
        tf("rank.unit_mib", &[("n", &format!("{:.0}", mib))])
    } else {
        tf("rank.unit_gib", &[("n", &format!("{:.1}", gib))])
    }
}

// ── ساخت متن پنل اصلی ────────────────────────────────────────────────────────
async fn build_main_text(db: &crate::database::postgresql::PostgresDatabase, user_id: i64) -> String {
    let client = db.client();
    let rank_row = get_user_rank(client, user_id).await.ok().flatten();
    let rank = rank_row.as_ref().map(|r| r.rank).unwrap_or(Rank::Dalavar);
    let expires_at = rank_row.as_ref().and_then(|r| r.expires_at);

    let expiry_line = match expires_at {
        Some(ts) => {
            let left = days_left(ts);
            tf("rank.expiry_with_date", &[("date", &fmt_jalali(ts)), ("days", &left.to_string())])
        }
        None => t("rank.expiry_unlimited"),
    };

    // ترافیک
    let daily_used = quota::get_daily_traffic(client, user_id).await.unwrap_or(0) as u64;
    let daily_limit = rank.daily_traffic_bytes();
    let daily_left = daily_limit.saturating_sub(daily_used);

    let first_upload = quota::get_first_upload_at(client, user_id).await;
    let monthly_used = if let Some(fu) = first_upload {
        quota::get_monthly_traffic(client, user_id, fu).await.unwrap_or(0) as u64
    } else {
        0
    };
    let monthly_limit = rank.monthly_traffic_bytes();
    let monthly_left = monthly_limit.saturating_sub(monthly_used);

    // هوش مصنوعی (تومار)
    let ai_used = quota::get_usage(client, user_id, quota::QuotaKind::AiChatMonthly, 30 * 86400)
        .await.unwrap_or(0) as u64;
    let ai_allowed = rank.ai_chat_monthly_toomar().is_some();
    let ai_limit = rank.ai_chat_monthly_toomar().unwrap_or(0) as u64;

    let rank_name = rank.display_name();
    apply_premium_to_html(&tf("panel.main_text", &[
        ("rank", &rank_name),
        ("expiry", &expiry_line),
        ("bar_d", &bar(daily_used, daily_limit, true)),
        ("used_d", &fmt_gib(daily_used)),
        ("left_d", &fmt_gib(daily_left)),
        ("bar_m", &bar(monthly_used, monthly_limit, true)),
        ("used_m", &fmt_gib(monthly_used)),
        ("left_m", &fmt_gib(monthly_left)),
        ("bar_ai", &bar(ai_used, ai_limit, ai_allowed)),
        ("ai_u", &ai_used.to_string()),
        ("ai_lim", &if ai_allowed { ai_limit.to_string() } else { "—".to_string() }),
    ]))
}

fn main_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                btn_icon(&t("panel.more_button"), CB_USER_PANEL_MORE, "stats"),
                btn_icon_success(&t("rank.paywall_button"), crate::rank::paywall::CB_RANK_SHOW_MENU, "rocket"),
            ],
            vec![btn_icon(&t("start.back"), crate::bot::CB_START_PANEL, "back")],
        ])
        .build()
}

fn back_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon(&t("panel.back_button"), CB_USER_PANEL, "back")],
            vec![btn_icon(&t("start.back"), crate::bot::CB_START_PANEL, "back")],
        ])
        .build()
}

// ── ساخت متن صفحه سهمیه‌های دیگر ─────────────────────────────────────────────
async fn build_more_text(db: &crate::database::postgresql::PostgresDatabase, user_id: i64) -> String {
    let client = db.client();
    let rank_row = get_user_rank(client, user_id).await.ok().flatten();
    let rank = rank_row.as_ref().map(|r| r.rank).unwrap_or(Rank::Dalavar);

    let week = 7 * 86400_i64;
    let day = 86400_i64;

    // STT سریع
    let stt_fast_d = quota::get_usage(client, user_id, quota::QuotaKind::SttFastDaily, day).await.unwrap_or(0) as u64;
    let stt_fast_w = quota::get_usage(client, user_id, quota::QuotaKind::SttFastWeekly, week).await.unwrap_or(0) as u64;
    let stt_fast_d_lim = rank.stt_fast_daily_secs().unwrap_or(0);
    let stt_fast_w_lim = rank.stt_fast_weekly_secs().unwrap_or(0);
    let stt_fast_allowed = rank.stt_fast_daily_secs().is_some();

    // STT دقیق
    let stt_acc_d = quota::get_usage(client, user_id, quota::QuotaKind::SttAccurateDaily, day).await.unwrap_or(0) as u64;
    let stt_acc_w = quota::get_usage(client, user_id, quota::QuotaKind::SttAccurateWeekly, week).await.unwrap_or(0) as u64;
    let stt_acc_d_lim = rank.stt_accurate_daily_secs().unwrap_or(0);
    let stt_acc_w_lim = rank.stt_accurate_weekly_secs().unwrap_or(0);
    let stt_acc_allowed = rank.stt_accurate_daily_secs().is_some();

    // حذف نویز
    let dn_d = quota::get_usage(client, user_id, quota::QuotaKind::DenoiseDaily, day).await.unwrap_or(0) as u64;
    let dn_w = quota::get_usage(client, user_id, quota::QuotaKind::DenoiseWeekly, week).await.unwrap_or(0) as u64;
    let dn_d_lim = rank.denoise_daily_secs();
    let dn_w_lim = rank.denoise_weekly_secs();

    // جداسازی
    let sep_d = quota::get_usage(client, user_id, quota::QuotaKind::SeparationDaily, day).await.unwrap_or(0) as u64;
    let sep_w = quota::get_usage(client, user_id, quota::QuotaKind::SeparationWeekly, week).await.unwrap_or(0) as u64;
    let sep_d_lim = rank.separation_daily_secs();
    let sep_w_lim = rank.separation_weekly_secs();

    // افزایش کیفیت
    let up2 = quota::get_usage(client, user_id, quota::QuotaKind::Upscale2xWeekly, week).await.unwrap_or(0) as u64;
    let up3 = quota::get_usage(client, user_id, quota::QuotaKind::Upscale3xWeekly, week).await.unwrap_or(0) as u64;
    let up4 = quota::get_usage(client, user_id, quota::QuotaKind::Upscale4xWeekly, week).await.unwrap_or(0) as u64;
    let up2_lim = rank.upscale_weekly_quota(2) as u64;
    let up3_lim = rank.upscale_weekly_quota(3) as u64;
    let up4_lim = rank.upscale_weekly_quota(4) as u64;

    fn fmt_secs(s: u64) -> String {
        if s >= 3600 { tf("rank.duration_hours", &[("hours", &format!("{:.1}", s as f64 / 3600.0))]) }
        else { tf("rank.duration_minutes", &[("mins", &(s / 60).to_string())]) }
    }

    apply_premium_to_html(&tf("panel.more_text", &[
        ("b_sf_d", &bar(stt_fast_d, stt_fast_d_lim, stt_fast_allowed)),
        ("u_sf_d", &fmt_secs(stt_fast_d)), ("l_sf_d", &fmt_secs(stt_fast_d_lim)),
        ("b_sf_w", &bar(stt_fast_w, stt_fast_w_lim, stt_fast_allowed)),
        ("u_sf_w", &fmt_secs(stt_fast_w)), ("l_sf_w", &fmt_secs(stt_fast_w_lim)),
        ("b_sa_d", &bar(stt_acc_d, stt_acc_d_lim, stt_acc_allowed)),
        ("u_sa_d", &fmt_secs(stt_acc_d)), ("l_sa_d", &fmt_secs(stt_acc_d_lim)),
        ("b_sa_w", &bar(stt_acc_w, stt_acc_w_lim, stt_acc_allowed)),
        ("u_sa_w", &fmt_secs(stt_acc_w)), ("l_sa_w", &fmt_secs(stt_acc_w_lim)),
        ("b_dn_d", &bar(dn_d, dn_d_lim, true)), ("u_dn_d", &fmt_secs(dn_d)), ("l_dn_d", &fmt_secs(dn_d_lim)),
        ("b_dn_w", &bar(dn_w, dn_w_lim, true)), ("u_dn_w", &fmt_secs(dn_w)), ("l_dn_w", &fmt_secs(dn_w_lim)),
        ("b_sep_d", &bar(sep_d, sep_d_lim, true)), ("u_sep_d", &fmt_secs(sep_d)), ("l_sep_d", &fmt_secs(sep_d_lim)),
        ("b_sep_w", &bar(sep_w, sep_w_lim, true)), ("u_sep_w", &fmt_secs(sep_w)), ("l_sep_w", &fmt_secs(sep_w_lim)),
        ("b_u2", &bar(up2, up2_lim, true)), ("v_u2", &up2.to_string()), ("l_u2", &up2_lim.to_string()),
        ("b_u3", &bar(up3, up3_lim, true)), ("v_u3", &up3.to_string()), ("l_u3", &up3_lim.to_string()),
        ("b_u4", &bar(up4, up4_lim, true)), ("v_u4", &up4.to_string()), ("l_u4", &up4_lim.to_string()),
    ]))
}

// ── public API ───────────────────────────────────────────────────────────────

pub async fn send_user_panel(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    database: &Option<PostgresDatabase>,
) {
    let Some(db) = database else {
        let _ = api.send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(t("panel.unavailable"))
                .build(),
        ).await;
        return;
    };
    let text = build_main_text(db, user_id).await;
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .parse_mode(frankenstein::ParseMode::Html)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(main_keyboard()))
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[panel event=send_failed] chat_id={chat_id} err={e}");
    }
}

pub async fn handle_panel_callback(
    api: &Bot,
    cq: &CallbackQuery,
    user_id: i64,
    database: &Option<PostgresDatabase>,
) {
    let _ = api.answer_callback_query(
        &AnswerCallbackQueryParams::builder().callback_query_id(cq.id.clone()).build(),
    ).await;

    let Some(msg) = cq.message.as_ref() else { return };
    let msg_id = match msg {
        frankenstein::types::MaybeInaccessibleMessage::Message(m) => m.message_id,
        _ => return,
    };
    let chat_id = match msg {
        frankenstein::types::MaybeInaccessibleMessage::Message(m) => m.chat.id,
        _ => return,
    };

    let cb = cq.data.as_deref().unwrap_or("");
    let Some(db) = database else { return };

    let (text, kb) = if cb == CB_USER_PANEL_MORE {
        (build_more_text(db, user_id).await, back_keyboard())
    } else {
        (build_main_text(db, user_id).await, main_keyboard())
    };

    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(msg_id)
        .text(&text)
        .parse_mode(frankenstein::ParseMode::Html)
        .reply_markup(kb)
        .build();
    if let Err(e) = api.edit_message_text(&params).await {
        eprintln!("[panel event=edit_failed] chat_id={chat_id} err={e}");
    }
}
