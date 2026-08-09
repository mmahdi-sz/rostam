use chrono::{Datelike, TimeZone, Timelike, Utc};
use chrono_tz::Asia::Tehran;
use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup},
};
use tokio_postgres::Client;

use crate::bot::send_text;
use crate::config;
use crate::database::postgresql::PostgresDatabase;
use crate::i18n::{apply_premium_to_md, md_escape, t, tf, to_fa_digits};
use crate::rank::types::Rank;
use crate::youtube::jalali::gregorian_to_jalali;

use super::generate::{parse_gen_args, random_code};
use super::panel::{build_keyboard, panel_text};
use super::panel_state::{self, GenSelection};
use super::store;

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Jalali datetime string (+3:30 Tehran time).
fn datetime_fa(epoch: i64) -> String {
    let Some(utc) = Utc.timestamp_opt(epoch, 0).single() else {
        return String::new();
    };
    let dt = utc.with_timezone(&Tehran);
    let (jy, jm, jd) = gregorian_to_jalali(dt.year(), dt.month() as i32, dt.day() as i32);
    format!(
        "{jy:04}/{jm:02}/{jd:02} hour {:02}:{:02}",
        dt.hour(),
        dt.minute()
    )
}

/// Jalali date string (+3:30 Tehran time) for expiration display.
fn date_fa(epoch: i64) -> String {
    let Some(utc) = Utc.timestamp_opt(epoch, 0).single() else {
        return String::new();
    };
    let dt = utc.with_timezone(&Tehran);
    let (jy, jm, jd) = gregorian_to_jalali(dt.year(), dt.month() as i32, dt.day() as i32);
    to_fa_digits(&format!("{jy:04}/{jm:02}/{jd:02}"))
}

use crate::rank::types::ceil_div;

/// Rank activation plan after evaluating user's current rank.
enum Plan {
    /// Rank lower than current rank -> reject code.
    Reject,
    /// Apply rank (`expires_at = None` means permanent).
    Apply {
        rank: Rank,
        expires_at: Option<i64>,
        total_days: Option<i64>,
    },
}

/// Calculates plan based on user's current rank and new code rank/duration.
async fn plan_redeem(client: &Client, user_id: i64, new_rank: Rank, new_days: i32) -> Plan {
    let now = now_epoch();
    let new_days = new_days as i64;
    let user_rank = crate::rank::store::get_user_rank(client, user_id)
        .await
        .ok()
        .flatten();

    let Some(cur) = user_rank else {
        let total = new_days;
        return Plan::Apply {
            rank: new_rank,
            expires_at: Some(now + total * 86_400),
            total_days: Some(total),
        };
    };

    let active = match cur.expires_at {
        Some(exp) => exp > now,
        None => true,
    };

    if !active {
        let total = new_days;
        return Plan::Apply {
            rank: new_rank,
            expires_at: Some(now + total * 86_400),
            total_days: Some(total),
        };
    }
    let wc = cur.rank.weight();
    let wn = new_rank.weight();

    // Lower rank -> reject
    if wn < wc {
        return Plan::Reject;
    }

    // Current rank is permanent -> new rank stays permanent
    let Some(cur_exp) = cur.expires_at else {
        return Plan::Apply {
            rank: new_rank,
            expires_at: None,
            total_days: None,
        };
    };

    let remaining_days = ceil_div((cur_exp - now).max(0), 86_400);

    // Equal rank -> full add; Upgrade -> weight-proportionally convert remaining days
    let converted = if wn == wc {
        remaining_days
    } else {
        ceil_div(remaining_days.saturating_mul(wc), wn)
    };
    let total = new_days + converted;
    Plan::Apply {
        rank: new_rank,
        expires_at: Some(now + total * 86_400),
        total_days: Some(total),
    }
}

// ──────────────────────────────── Code Generation Admin Panel ────────────────────────────────

/// Opens code generation panel by editing the admin panel message in place.
pub async fn open_panel_edit(api: &Bot, chat_id: i64, message_id: i32, admin_id: i64) {
    let sel = GenSelection::default();
    panel_state::save(admin_id, sel).await;
    refresh_panel(api, chat_id, message_id, &sel).await;
}

/// Refreshes panel message after selection changes.
async fn refresh_panel(api: &Bot, chat_id: i64, message_id: i32, sel: &GenSelection) {
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(panel_text(sel))
        .reply_markup(build_keyboard(sel))
        .build();
    if let Err(e) = api.edit_message_text(&params).await {
        let desc = e.to_string();
        if !desc.contains("message is not modified") {
            eprintln!("[redeem event=panel_refresh_failed err={desc}]");
        }
    }
}

/// Handles panel button callbacks (`gc:*`).
pub async fn handle_panel_callback(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    admin_id: i64,
    data: &str,
    database: &Option<PostgresDatabase>,
) {
    use super::panel::{CB_GC_DAYS, CB_GC_GO, CB_GC_RANK, CB_GC_USES};

    if data == CB_GC_GO {
        let sel = panel_state::load(admin_id).await;
        panel_state::clear(admin_id).await;
        do_generate(
            api, chat_id, admin_id, sel.rank, sel.days, sel.uses, database,
        )
        .await;
        return;
    }

    let mut sel = panel_state::load(admin_id).await;
    let mut changed = false;

    if let Some(r) = data.strip_prefix(CB_GC_RANK) {
        if let Some(rank) = Rank::from_str(r) {
            if sel.rank != rank {
                sel.rank = rank;
                changed = true;
            }
        }
    } else if let Some(d) = data.strip_prefix(CB_GC_DAYS) {
        if let Ok(days) = d.parse::<i32>() {
            if sel.days != days {
                sel.days = days;
                changed = true;
            }
        }
    } else if let Some(u) = data.strip_prefix(CB_GC_USES) {
        if let Ok(uses) = u.parse::<i32>() {
            if sel.uses != uses {
                sel.uses = uses;
                changed = true;
            }
        }
    }

    if changed {
        panel_state::save(admin_id, sel).await;
        refresh_panel(api, chat_id, message_id, &sel).await;
    }
}

// ──────────────────────────────── Code Generation ────────────────────────────────

/// Shared code generation logic (panel & `/re` command).
async fn do_generate(
    api: &Bot,
    chat_id: i64,
    created_by: i64,
    rank: Rank,
    days: i32,
    uses: i32,
    database: &Option<PostgresDatabase>,
) {
    let Some(db) = database else {
        let _ = send_text(api, chat_id, &t("redeem.db_missing")).await;
        return;
    };

    let code = random_code();
    if let Err(e) = store::create_code(db.client(), &code, rank, days, uses, created_by).await {
        eprintln!("[redeem event=create_failed code={code} err={e}]");
        let _ = send_text(api, chat_id, &t("redeem.gen_error")).await;
        return;
    };

    eprintln!(
        "[redeem event=created code={code} rank={} days={days} uses={uses} by={created_by}]",
        rank.as_str()
    );

    let username = config::bot_username();
    let link = if username.is_empty() {
        String::new()
    } else {
        format!("https://t.me/{username}?start=redeem{code}")
    };

    let _ = uses;
    let rank_name = rank.display_name();
    let msg = tf(
        "redeem.created",
        &[
            ("rank", &rank_name),
            ("days", &to_fa_digits(&days.to_string())),
            ("link", &link),
        ],
    );
    let _ = send_text(api, chat_id, &msg).await;
}

/// Generates code via text command `/re 30d es 1u` (admin only).
pub async fn handle_generate(
    api: &Bot,
    chat_id: i64,
    created_by: i64,
    args: &str,
    database: &Option<PostgresDatabase>,
) {
    match parse_gen_args(args) {
        Ok((rank, days, uses)) => {
            do_generate(api, chat_id, created_by, rank, days, uses, database).await;
        }
        Err(e) => {
            let _ = send_text(api, chat_id, &tf("redeem.gen_bad_args", &[("err", &e)])).await;
        }
    }
}

// ──────────────────────────────── User Redemption ────────────────────────────────

/// Back button inline keyboard for main menu.
fn back_keyboard() -> InlineKeyboardMarkup {
    let btn = InlineKeyboardButton {
        text: t("redeem.back_button"),
        icon_custom_emoji_id: None,
        callback_data: Some(crate::bot::CB_START_PANEL.to_string()),
        style: Some(ButtonStyle::Primary),
        url: None,
        login_url: None,
        web_app: None,
        switch_inline_query: None,
        switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None,
        copy_text: None,
        callback_game: None,
        pay: None,
    };
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn]])
        .build()
}

/// Sends HTML message with back button.
async fn send_with_back(api: &Bot, chat_id: i64, text: &str) {
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .parse_mode(ParseMode::Html)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(back_keyboard()))
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[redeem event=msg_send_failed chat_id={chat_id} err={e}]");
    }
}

/// Redeems code for user (start deep-link `redeem<CODE>`). Returns `true` if successful.
pub async fn handle_redeem(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    first_name: &str,
    username: Option<&str>,
    code: &str,
    database: &Option<PostgresDatabase>,
) -> bool {
    let code = code.trim();
    let Some(db) = database else {
        send_with_back(api, chat_id, &t("redeem.invalid")).await;
        return false;
    };
    let client = db.client();

    let row = match store::get_code(client, code).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            eprintln!("[redeem event=redeem_invalid user_id={user_id} code={code}]");
            send_with_back(api, chat_id, &t("redeem.invalid")).await;
            return false;
        }
        Err(e) => {
            eprintln!("[redeem event=get_failed code={code} err={e}]");
            send_with_back(api, chat_id, &t("redeem.invalid")).await;
            return false;
        }
    };

    // Lazy expiration check
    if let Some(exp) = row.expires_at {
        if now_epoch() > exp {
            let _ = store::delete_code(client, code).await;
            eprintln!("[redeem event=redeem_expired user_id={user_id} code={code}]");
            send_with_back(api, chat_id, &t("redeem.invalid")).await;
            return false;
        }
    }

    match store::get_user_redemption(client, code, user_id).await {
        Ok(Some(ts)) => {
            eprintln!("[redeem event=already_used user_id={user_id} code={code}]");
            let msg = tf("redeem.consumed", &[("datetime", &datetime_fa(ts))]);
            send_with_back(api, chat_id, &msg).await;
            return false;
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("[redeem event=user_redemption_failed code={code} err={e}]");
            send_with_back(api, chat_id, &t("redeem.invalid")).await;
            return false;
        }
    }

    let (apply_rank, apply_expires, apply_total) =
        match plan_redeem(client, user_id, row.rank, row.duration_days).await {
            Plan::Reject => {
                eprintln!(
                    "[redeem event=downgrade_reject user_id={user_id} code={code} code_rank={}]",
                    row.rank.as_str()
                );
                send_with_back(api, chat_id, &t("redeem.downgrade")).await;
                return false;
            }
            Plan::Apply {
                rank,
                expires_at,
                total_days,
            } => (rank, expires_at, total_days),
        };

    match store::mark_redeemed(client, code, user_id).await {
        Ok(store::RedeemOutcome::Consumed) => {}
        Ok(store::RedeemOutcome::AlreadyRedeemed) => {
            eprintln!("[redeem event=already_used user_id={user_id} code={code}]");
            let ts = store::get_user_redemption(client, code, user_id)
                .await
                .ok()
                .flatten()
                .unwrap_or_else(now_epoch);
            let msg = tf("redeem.consumed", &[("datetime", &datetime_fa(ts))]);
            send_with_back(api, chat_id, &msg).await;
            return false;
        }
        Ok(store::RedeemOutcome::Exhausted) => {
            eprintln!("[redeem event=exhausted user_id={user_id} code={code}]");
            let last = store::get_last_redemption(client, code)
                .await
                .ok()
                .flatten()
                .unwrap_or_else(now_epoch);
            let msg = tf("redeem.consumed", &[("datetime", &datetime_fa(last))]);
            send_with_back(api, chat_id, &msg).await;
            return false;
        }
        Err(e) => {
            eprintln!("[redeem event=mark_failed code={code} err={e}]");
            send_with_back(api, chat_id, &t("redeem.invalid")).await;
            return false;
        }
    }

    if let Err(e) =
        crate::rank::store::set_user_rank(client, user_id, apply_rank, apply_expires).await
    {
        eprintln!("[redeem event=apply_failed user_id={user_id} code={code} err={e}]");
        let _ = send_text(api, chat_id, &t("redeem.apply_error")).await;
        return false;
    }

    eprintln!(
        "[redeem event=redeem_ok user_id={user_id} code={code} rank={} total_days={:?} expires={:?}]",
        apply_rank.as_str(),
        apply_total,
        apply_expires
    );
    crate::stats::record_event_user(user_id, "rank", "redeem", "ok", 0).await;

    let apply_rank_name = apply_rank.display_name();
    let msg = match (apply_expires, apply_total) {
        (Some(exp), Some(days)) => tf(
            "redeem.success",
            &[
                ("rank", &apply_rank_name),
                ("date", &date_fa(exp)),
                ("days", &to_fa_digits(&days.to_string())),
            ],
        ),
        _ => tf("redeem.success_unlimited", &[("rank", &apply_rank_name)]),
    };
    let _ = send_text(api, chat_id, &msg).await;

    // Admin notification
    if let Some(admin_id) = config::admin_user_id() {
        let username_display = username
            .map(|u| md_escape(&format!("@{u}")))
            .unwrap_or_else(|| t("redeem.no_username"));
        let duration_display = apply_total
            .map(|d| md_escape(&tf("redeem.panel_days", &[("n", &d.to_string())])))
            .unwrap_or_else(|| t("rank.expiry_unlimited"));
        let apply_rank_name = apply_rank.display_name();
        let raw_msg = tf(
            "redeem.admin_notify",
            &[
                ("code", &md_escape(code)),
                ("name", &md_escape(first_name)),
                ("username", &username_display),
                ("user_id", &user_id.to_string()),
                ("rank", &md_escape(&apply_rank_name)),
                ("duration", &duration_display),
                ("time", &md_escape(&datetime_fa(now_epoch()))),
            ],
        );
        let admin_msg = apply_premium_to_md(&raw_msg);
        let params = SendMessageParams::builder()
            .chat_id(admin_id)
            .text(admin_msg)
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        if let Err(e) = api.send_message(&params).await {
            eprintln!("[redeem event=admin_notify_failed admin_id={admin_id} err={e}]");
        }
    }
    true
}
