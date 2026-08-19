use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{AnswerCallbackQueryParams, EditMessageTextParams, SendMessageParams},
    types::{CallbackQuery, InlineKeyboardMarkup, ReplyMarkup},
};

use super::quota;
use super::store::get_user_rank;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::panel::{btn_icon, btn_icon_primary, btn_icon_success};
use crate::i18n::{apply_premium_to_html, t, tf};

pub const CB_USER_PANEL: &str = "user:panel";
pub const CB_USER_PANEL_MORE: &str = "user:panel:more";
pub const CB_REFERRAL: &str = "user:panel:referral";
pub const CB_REFERRAL_CLAIM_PREFIX: &str = "user:panel:referral:claim:";

// ── Convert epoch → Jalali, Tehran time ───────────────────────────────────────
// Uses chrono + gregorian_to_jalali.
fn fmt_jalali(epoch: i64) -> String {
    use chrono::Datelike;
    use chrono_tz::Asia::Tehran;
    let Some(utc) = chrono::DateTime::from_timestamp(epoch, 0) else {
        return "—".to_string();
    };
    let dt = utc.with_timezone(&Tehran);
    let (y, m, d) =
        crate::youtube::jalali::gregorian_to_jalali(dt.year(), dt.month() as i32, dt.day() as i32);
    format!("{y}/{m:02}/{d:02}")
}

fn fmt_expiry_line(expires_at: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let date_str = fmt_jalali(expires_at);
    if now > expires_at {
        tf("rank.expiry_expired", &[("date", &date_str)])
    } else {
        let diff = expires_at - now;
        if diff >= 86_400 {
            let days = diff / 86_400;
            tf(
                "rank.expiry_with_date",
                &[("date", &date_str), ("days", &days.to_string())],
            )
        } else {
            let hours = (diff / 3600).max(1);
            tf(
                "rank.expiry_with_hours",
                &[("date", &date_str), ("hours", &hours.to_string())],
            )
        }
    }
}

// ── Progress bar ──────────────────────────────────────────────────────────────
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
    format!("{filled}{empty}")
}

fn fmt_gib(bytes: u64) -> String {
    let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gib < 1.0 {
        let mib = bytes as f64 / (1024.0 * 1024.0);
        tf("rank.unit_mib", &[("n", &format!("{mib:.0}"))])
    } else {
        tf("rank.unit_gib", &[("n", &format!("{gib:.1}"))])
    }
}

// ── Build main panel text ─────────────────────────────────────────────────────
async fn build_main_text(
    db: &crate::database::postgresql::PostgresDatabase,
    user_id: i64,
) -> String {
    let client = match db.get().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[panel event=db_get_failed err={e}]");
            return t("panel.unavailable");
        }
    };
    let rank = super::effective_rank(&client, user_id).await;
    let rank_row = get_user_rank(&client, user_id).await.ok().flatten();
    let expires_at = rank_row.as_ref().and_then(|r| r.expires_at);

    let expiry_line = match expires_at {
        Some(ts) => fmt_expiry_line(ts),
        None => t("rank.expiry_unlimited"),
    };

    // Traffic
    let daily_used = quota::get_daily_traffic(&client, user_id).await.unwrap_or(0) as u64;
    let daily_limit = rank.daily_traffic_bytes();
    let daily_left = daily_limit.saturating_sub(daily_used);

    let first_upload = quota::get_first_upload_at(&client, user_id).await;
    let monthly_used = if let Some(fu) = first_upload {
        quota::get_monthly_traffic(&client, user_id, fu)
            .await
            .unwrap_or(0) as u64
    } else {
        0
    };
    let monthly_limit = rank.monthly_traffic_bytes();
    let monthly_left = monthly_limit.saturating_sub(monthly_used);

    let rank_name = rank.display_name();
    apply_premium_to_html(&tf(
        "panel.main_text",
        &[
            ("rank", &rank_name),
            ("expiry", &expiry_line),
            ("bar_d", &bar(daily_used, daily_limit, true)),
            ("used_d", &fmt_gib(daily_used)),
            ("left_d", &fmt_gib(daily_left)),
            ("bar_m", &bar(monthly_used, monthly_limit, true)),
            ("used_m", &fmt_gib(monthly_used)),
            ("left_m", &fmt_gib(monthly_left)),
        ],
    ))
}

fn main_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                btn_icon(&t("panel.more_button"), CB_USER_PANEL_MORE, "stats"),
                btn_icon_success(
                    &t("rank.paywall_button"),
                    crate::rank::paywall::CB_RANK_SHOW_MENU,
                    "rocket",
                ),
            ],
            vec![btn_icon_success(&t("referral.button"), CB_REFERRAL, "user")],
            vec![btn_icon_primary(
                &t("start.back"),
                crate::bot::CB_START_PANEL,
                "back",
            )],
        ])
        .build()
}

fn back_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon(&t("panel.back_button"), CB_USER_PANEL, "back")],
            vec![btn_icon_primary(
                &t("start.back"),
                crate::bot::CB_START_PANEL,
                "back",
            )],
        ])
        .build()
}

/// Persistent chat message with back buttons for permanent result viewing.
async fn send_with_back(api: &Bot, chat_id: i64, text: &str) {
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(apply_premium_to_html(text))
        .parse_mode(frankenstein::ParseMode::Html)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(back_keyboard()))
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[panel event=claim_result_send_failed] chat_id={chat_id} err={e}");
    }
}

// ── Build quota details page text ─────────────────────────────────────────────
async fn build_more_text(
    db: &crate::database::postgresql::PostgresDatabase,
    user_id: i64,
) -> String {
    let client = match db.get().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[panel event=db_get_failed err={e}]");
            return t("panel.unavailable");
        }
    };
    let rank = super::effective_rank(&client, user_id).await;

    let week = 7 * 86400_i64;
    let day = 86400_i64;

    // Fast STT
    let stt_fast_d = quota::get_usage(&client, user_id, quota::QuotaKind::SttFastDaily, day)
        .await
        .unwrap_or(0) as u64;
    let stt_fast_w = quota::get_usage(&client, user_id, quota::QuotaKind::SttFastWeekly, week)
        .await
        .unwrap_or(0) as u64;
    let stt_fast_d_lim = rank.stt_fast_daily_secs().unwrap_or(0);
    let stt_fast_w_lim = rank.stt_fast_weekly_secs().unwrap_or(0);
    let stt_fast_allowed = rank.stt_fast_daily_secs().is_some();

    // Accurate STT
    let stt_acc_d = quota::get_usage(&client, user_id, quota::QuotaKind::SttAccurateDaily, day)
        .await
        .unwrap_or(0) as u64;
    let stt_acc_w = quota::get_usage(&client, user_id, quota::QuotaKind::SttAccurateWeekly, week)
        .await
        .unwrap_or(0) as u64;
    let stt_acc_d_lim = rank.stt_accurate_daily_secs().unwrap_or(0);
    let stt_acc_w_lim = rank.stt_accurate_weekly_secs().unwrap_or(0);
    let stt_acc_allowed = rank.stt_accurate_daily_secs().is_some();

    // Denoise
    let dn_d = quota::get_usage(&client, user_id, quota::QuotaKind::DenoiseDaily, day)
        .await
        .unwrap_or(0) as u64;
    let dn_w = quota::get_usage(&client, user_id, quota::QuotaKind::DenoiseWeekly, week)
        .await
        .unwrap_or(0) as u64;
    let dn_d_lim = rank.denoise_daily_secs();
    let dn_w_lim = rank.denoise_weekly_secs();

    // Separation
    let sep_d = quota::get_usage(&client, user_id, quota::QuotaKind::SeparationDaily, day)
        .await
        .unwrap_or(0) as u64;
    let sep_w = quota::get_usage(&client, user_id, quota::QuotaKind::SeparationWeekly, week)
        .await
        .unwrap_or(0) as u64;
    let sep_d_lim = rank.separation_daily_secs();
    let sep_w_lim = rank.separation_weekly_secs();

    // Upscale
    let up2 = quota::get_usage(&client, user_id, quota::QuotaKind::Upscale2xWeekly, week)
        .await
        .unwrap_or(0) as u64;
    let up3 = quota::get_usage(&client, user_id, quota::QuotaKind::Upscale3xWeekly, week)
        .await
        .unwrap_or(0) as u64;
    let up4 = quota::get_usage(&client, user_id, quota::QuotaKind::Upscale4xWeekly, week)
        .await
        .unwrap_or(0) as u64;
    let up2_lim = rank.upscale_weekly_quota(2) as u64;
    let up3_lim = rank.upscale_weekly_quota(3) as u64;
    let up4_lim = rank.upscale_weekly_quota(4) as u64;

    // NoBg
    let nobg = quota::get_usage(&client, user_id, quota::QuotaKind::NobgWeekly, week)
        .await
        .unwrap_or(0) as u64;
    let nobg_lim = rank.nobg_weekly_quota() as u64;

    // DeOldify
    let deoldify = quota::get_usage(&client, user_id, quota::QuotaKind::DeoldifyWeekly, week)
        .await
        .unwrap_or(0) as u64;
    let deoldify_lim = rank.deoldify_weekly_quota() as u64;

    // TTS
    let tts = quota::get_usage(&client, user_id, quota::QuotaKind::TtsWeekly, week)
        .await
        .unwrap_or(0) as u64;
    let tts_lim = rank.tts_weekly_secs();

    fn fmt_secs(s: u64) -> String {
        if s >= 3600 {
            tf(
                "rank.duration_hours",
                &[("hours", &format!("{:.1}", s as f64 / 3600.0))],
            )
        } else {
            tf("rank.duration_minutes", &[("mins", &(s / 60).to_string())])
        }
    }

    apply_premium_to_html(&tf(
        "panel.more_text",
        &[
            ("b_sf_d", &bar(stt_fast_d, stt_fast_d_lim, stt_fast_allowed)),
            ("u_sf_d", &fmt_secs(stt_fast_d)),
            ("l_sf_d", &fmt_secs(stt_fast_d_lim)),
            ("b_sf_w", &bar(stt_fast_w, stt_fast_w_lim, stt_fast_allowed)),
            ("u_sf_w", &fmt_secs(stt_fast_w)),
            ("l_sf_w", &fmt_secs(stt_fast_w_lim)),
            ("b_sa_d", &bar(stt_acc_d, stt_acc_d_lim, stt_acc_allowed)),
            ("u_sa_d", &fmt_secs(stt_acc_d)),
            ("l_sa_d", &fmt_secs(stt_acc_d_lim)),
            ("b_sa_w", &bar(stt_acc_w, stt_acc_w_lim, stt_acc_allowed)),
            ("u_sa_w", &fmt_secs(stt_acc_w)),
            ("l_sa_w", &fmt_secs(stt_acc_w_lim)),
            ("b_dn_d", &bar(dn_d, dn_d_lim, true)),
            ("u_dn_d", &fmt_secs(dn_d)),
            ("l_dn_d", &fmt_secs(dn_d_lim)),
            ("b_dn_w", &bar(dn_w, dn_w_lim, true)),
            ("u_dn_w", &fmt_secs(dn_w)),
            ("l_dn_w", &fmt_secs(dn_w_lim)),
            ("b_sep_d", &bar(sep_d, sep_d_lim, true)),
            ("u_sep_d", &fmt_secs(sep_d)),
            ("l_sep_d", &fmt_secs(sep_d_lim)),
            ("b_sep_w", &bar(sep_w, sep_w_lim, true)),
            ("u_sep_w", &fmt_secs(sep_w)),
            ("l_sep_w", &fmt_secs(sep_w_lim)),
            ("b_u2", &bar(up2, up2_lim, true)),
            ("v_u2", &up2.to_string()),
            ("l_u2", &up2_lim.to_string()),
            ("b_u3", &bar(up3, up3_lim, true)),
            ("v_u3", &up3.to_string()),
            ("l_u3", &up3_lim.to_string()),
            ("b_u4", &bar(up4, up4_lim, true)),
            ("v_u4", &up4.to_string()),
            ("l_u4", &up4_lim.to_string()),
            ("b_nobg", &bar(nobg, nobg_lim, true)),
            ("u_nobg", &nobg.to_string()),
            ("l_nobg", &nobg_lim.to_string()),
            ("b_deold", &bar(deoldify, deoldify_lim, true)),
            ("u_deold", &deoldify.to_string()),
            ("l_deold", &deoldify_lim.to_string()),
            ("b_tts", &bar(tts, tts_lim, true)),
            ("u_tts", &fmt_secs(tts)),
            ("l_tts", &fmt_secs(tts_lim)),
        ],
    ))
}

// ── Referral ─────────────────────────────────────────────────────────────────

fn referral_keyboard() -> InlineKeyboardMarkup {
    let tier_rows = crate::referral::TIERS.iter().map(|(threshold, rank)| {
        let label = tf(
            "referral.tier_button",
            &[
                ("rank", &rank.display_name()),
                ("count", &threshold.to_string()),
            ],
        );
        vec![btn_icon(
            &label,
            &format!("{CB_REFERRAL_CLAIM_PREFIX}{threshold}"),
            "rocket",
        )]
    });
    let mut rows: Vec<Vec<_>> = tier_rows.collect();
    rows.push(vec![btn_icon(
        &t("panel.back_button"),
        CB_USER_PANEL,
        "back",
    )]);
    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

pub async fn send_referral(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    database: &Option<PostgresDatabase>,
) {
    let username = crate::config::bot_username();
    let banner = tf(
        "referral.banner",
        &[("username", username), ("user_id", &user_id.to_string())],
    );
    let _ = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(apply_premium_to_html(&banner))
                .parse_mode(frankenstein::ParseMode::Html)
                .link_preview_options(
                    frankenstein::types::LinkPreviewOptions::builder()
                        .is_disabled(true)
                        .build(),
                )
                .build(),
        )
        .await;

    let (count, available, pending) = if let Some(db) = database {
        if let Ok(client) = db.get().await {
            let total = crate::referral::count_referrals(&client, user_id).await;
            let spent = crate::referral::total_spent_points(&client, user_id).await;
            let pending = crate::referral::count_pending(&client, user_id).await;
            (total, total - spent, pending)
        } else {
            (0, 0, 0)
        }
    } else {
        (0, 0, 0)
    };
    let status = tf(
        "referral.status",
        &[
            ("count", &count.to_string()),
            ("available", &available.to_string()),
            ("pending", &pending.to_string()),
        ],
    );
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(apply_premium_to_html(&status))
        .parse_mode(frankenstein::ParseMode::Html)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(referral_keyboard()))
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[panel event=referral_send_failed] chat_id={chat_id} err={e}");
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

pub async fn send_user_panel(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    database: &Option<PostgresDatabase>,
) {
    let Some(db) = database else {
        let _ = api
            .send_message(
                &SendMessageParams::builder()
                    .chat_id(chat_id)
                    .text(t("panel.unavailable"))
                    .build(),
            )
            .await;
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

/// Activates rank using referral points; returns result text.
pub(crate) async fn process_claim(
    database: &Option<PostgresDatabase>,
    user_id: i64,
    threshold: u32,
    trace_id: u64,
) -> String {
    let Some(&(_, tier_rank)) = crate::referral::TIERS
        .iter()
        .find(|(th, _)| *th == threshold)
    else {
        log_ev!("referral", trace_id, "claim", "=>" => "unknown_tier");
        return t("panel.unavailable");
    };

    let Some(db) = database else {
        log_ev!("referral", trace_id, "claim", "=>" => "no_db");
        return t("panel.unavailable");
    };
    let mut client = match db.get().await {
        Ok(c) => c,
        Err(e) => {
            log_ev!("referral", trace_id, "claim", "=>" => "no_conn", "err" => e);
            return t("panel.unavailable");
        }
    };

    let total = crate::referral::count_referrals(&client, user_id).await;
    let spent = crate::referral::total_spent_points(&client, user_id).await;
    let available = total - spent;
    log_ev!("referral", trace_id, "points_check",
        "total" => total, "spent" => spent, "available" => available, "needed" => threshold);

    if available < threshold as i64 {
        log_ev!("referral", trace_id, "points_check", "=>" => "insufficient");
        return tf(
            "referral.activate_insufficient",
            &[
                ("available", &available.to_string()),
                ("needed", &threshold.to_string()),
            ],
        );
    }

    log_ev!("referral", trace_id, "plan_activation_enter");
    match crate::referral::plan_activation(&client, user_id, tier_rank).await {
        crate::referral::ActivationPlan::Reject => {
            log_ev!("referral", trace_id, "downgrade_check", "=>" => "rejected");
            t("referral.activate_downgrade")
        }
        crate::referral::ActivationPlan::AlreadyUnlimited => {
            log_ev!("referral", trace_id, "downgrade_check", "=>" => "already_unlimited");
            t("referral.activate_unlimited")
        }
        crate::referral::ActivationPlan::Apply { rank, expires_at } => {
            log_ev!("referral", trace_id, "rank_apply_enter", "rank" => rank.as_str(), "expires_at" => expires_at);
            let txn = match client.transaction().await {
                Ok(t) => t,
                Err(e) => {
                    log_ev!("referral", trace_id, "rank_apply", "=>" => "tx_start_fail", "err" => e);
                    return t("redeem.apply_error");
                }
            };
            if let Err(e) =
                crate::rank::store::set_user_rank(&*txn, user_id, rank, Some(expires_at)).await
            {
                let _ = txn.rollback().await;
                log_ev!("referral", trace_id, "rank_apply", "=>" => "fail", "err" => e);
                return t("redeem.apply_error");
            }
            // Maintain order: set rank first, then deduct points in the same transaction.
            match crate::referral::record_activation(
                &*txn,
                user_id,
                rank,
                threshold as i64,
                expires_at,
            )
            .await
            {
                Ok(true) => {
                    if let Err(e) = txn.commit().await {
                        log_ev!("referral", trace_id, "rank_apply", "=>" => "commit_fail", "err" => e);
                        return t("redeem.apply_error");
                    }
                    log_ev!("referral", trace_id, "rank_apply", "=>" => "ok");
                }
                // Concurrent second click: rollback so rank is not applied without debit
                Ok(false) => {
                    let _ = txn.rollback().await;
                    log_ev!("referral", trace_id, "rank_apply", "=>" => "ok_no_debit");
                }
                Err(e) => {
                    let _ = txn.rollback().await;
                    log_ev!("referral", trace_id, "rank_apply", "=>" => "debit_fail", "err" => e);
                    return t("redeem.apply_error");
                }
            }
            tf(
                "referral.activate_success",
                &[
                    ("rank", &rank.display_name()),
                    ("date", &fmt_jalali(expires_at)),
                ],
            )
        }
    }
}

pub async fn handle_panel_callback(
    api: &Bot,
    cq: &CallbackQuery,
    user_id: i64,
    database: &Option<PostgresDatabase>,
) {
    let cb = cq.data.as_deref().unwrap_or("");

    let claim_threshold = cb
        .strip_prefix(CB_REFERRAL_CLAIM_PREFIX)
        .and_then(|s| s.parse::<u32>().ok());
    let claim_result = if let Some(threshold) = claim_threshold {
        let trace_id = crate::log::next_trace_id();
        log_actor_id!("referral", trace_id, user_id, "clicked" => cb);
        Some(process_claim(database, user_id, threshold, trace_id).await)
    } else {
        None
    };

    // Empty ack for claim stops Telegram spinner; result sent as persistent message.
    let ack = AnswerCallbackQueryParams::builder()
        .callback_query_id(cq.id.clone())
        .build();
    let _ = api.answer_callback_query(&ack).await;

    let Some(msg) = cq.message.as_ref() else {
        return;
    };
    let chat_id = match msg {
        frankenstein::types::MaybeInaccessibleMessage::Message(m) => m.chat.id,
        _ => return,
    };

    if let Some(text) = claim_result {
        send_with_back(api, chat_id, &text).await;
        return;
    }
    if cb == CB_REFERRAL {
        send_referral(api, chat_id, user_id, database).await;
        return;
    }

    let msg_id = match msg {
        frankenstein::types::MaybeInaccessibleMessage::Message(m) => m.message_id,
        _ => return,
    };
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
