use frankenstein::client_reqwest::Bot;

use crate::database::postgresql::PostgresDatabase;
use crate::emoji::FlowManager;
use crate::i18n::tf;
use crate::rank::{
    self,
    quota::{QuotaKind, get_usage, refund_usage, reserve_usage},
};

use super::format::{delete_message, format_duration_fa};
use super::log_trace;

pub struct QuotaReservation {
    pub reserved: bool,
    pub reserve_secs: i64,
}

pub async fn probe_audio_duration(
    tmp_dir: &std::path::Path,
    audio_bytes: &[u8],
    trace_id: u64,
) -> u64 {
    let tmp_probe = tmp_dir.join("probe_audio");
    std::fs::write(&tmp_probe, audio_bytes).unwrap_or(());
    let audio_duration_secs = if let Some(tmp_probe_str) = tmp_probe.to_str() {
        let probe = tokio::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "csv=p=0",
                tmp_probe_str,
            ])
            .output()
            .await;
        probe
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map(|d| d.ceil() as u64)
            .unwrap_or(0)
    } else {
        0
    };
    std::fs::remove_file(&tmp_probe).ok();
    log_trace(
        trace_id,
        "duration_probed",
        &format!("secs={audio_duration_secs}"),
    );
    audio_duration_secs
}

pub async fn check_and_reserve_quota(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    database: &Option<PostgresDatabase>,
    flow_manager: &mut FlowManager,
    tmp_dir: &std::path::Path,
    audio_duration_secs: u64,
    trace_id: u64,
) -> Result<QuotaReservation, ()> {
    let mut reserved = false;
    let mut reserve_secs: i64 = 0;

    if let Some(db) = database.as_ref() {
        let (user_rank, daily_limit, weekly_limit) = {
            let client = match db.get().await {
                Ok(c) => c,
                Err(e) => {
                    log_trace(trace_id, "quota_checkout", &format!("err={e} => fail"));
                    let _ = delete_message(api, chat_id, message_id).await;
                    std::fs::remove_dir_all(tmp_dir).ok();
                    flow_manager.clear(user_id);
                    crate::rank::paywall::quota_db_error(api, chat_id, "separation", &format!("{e}")).await;
                    return Err(());
                }
            };
            let user_rank = rank::effective_rank(&client, user_id).await;
            let d_lim = user_rank.separation_daily_secs();
            let w_lim = user_rank.separation_weekly_secs();
            (user_rank, d_lim, w_lim)
        };

        reserve_secs = audio_duration_secs.max(1) as i64;

        let handle_deny = || async {
            let (d_used, w_used) = if let Ok(client) = db.get().await {
                let d = get_usage(&client, user_id, QuotaKind::SeparationDaily, 86400)
                    .await
                    .unwrap_or(0) as u64;
                let w = get_usage(&client, user_id, QuotaKind::SeparationWeekly, 7 * 86400)
                    .await
                    .unwrap_or(0) as u64;
                (d, w)
            } else {
                (0, 0)
            };
            let d_rem = daily_limit.saturating_sub(d_used);
            let w_rem = weekly_limit.saturating_sub(w_used);
            let label = if d_rem == 0 {
                tf(
                    "separation.quota_daily_limit",
                    &[("limit", &format_duration_fa(daily_limit))],
                )
            } else if w_rem == 0 {
                tf(
                    "separation.quota_weekly_limit",
                    &[("limit", &format_duration_fa(weekly_limit))],
                )
            } else {
                tf(
                    "separation.quota_file_too_long",
                    &[("remaining", &format_duration_fa(d_rem.min(w_rem)))],
                )
            };
            log_trace(
                trace_id,
                "quota_blocked",
                &format!("user_id={user_id} daily_used={d_used} weekly_used={w_used}"),
            );
            let _ = delete_message(api, chat_id, message_id).await;
            std::fs::remove_dir_all(tmp_dir).ok();
            flow_manager.clear(user_id);
            if let Some(min_rank) = user_rank.separation_next_rank() {
                crate::rank::paywall::block_limit(api, chat_id, &label, min_rank).await;
            } else {
                let _ = crate::bot::send_text_with_ai_back(api, chat_id, &label).await;
            }
        };

        let (daily_res, weekly_res) = {
            let client = match db.get().await {
                Ok(c) => c,
                Err(e) => {
                    log_trace(trace_id, "quota_reserve", &format!("err={e} => fail"));
                    let _ = delete_message(api, chat_id, message_id).await;
                    std::fs::remove_dir_all(tmp_dir).ok();
                    flow_manager.clear(user_id);
                    crate::rank::paywall::quota_db_error(
                        api,
                        chat_id,
                        "separation",
                        &format!("{e}"),
                    )
                    .await;
                    return Err(());
                }
            };
            let d_res = reserve_usage(
                &client,
                user_id,
                QuotaKind::SeparationDaily,
                reserve_secs,
                86400,
                daily_limit as i64,
            )
            .await;
            let w_res = if matches!(d_res, Ok(Some(_))) {
                let w = reserve_usage(
                    &client,
                    user_id,
                    QuotaKind::SeparationWeekly,
                    reserve_secs,
                    7 * 86400,
                    weekly_limit as i64,
                )
                .await;
                if !matches!(w, Ok(Some(_))) {
                    if let Err(e) = refund_usage(
                        &client,
                        user_id,
                        QuotaKind::SeparationDaily,
                        reserve_secs,
                        86400,
                    )
                    .await
                    {
                        log_trace(trace_id, "quota_refund_failed", &e.to_string());
                        crate::stats::record_error_global("separation", &format!("refund_failed: {e}")).await;
                    }
                }
                Some(w)
            } else {
                None
            };
            (d_res, w_res)
        };

        match daily_res {
            Ok(Some(used)) => log_trace(
                trace_id,
                "quota_reserved_daily",
                &format!("used={used} limit={daily_limit}"),
            ),
            Ok(None) => {
                handle_deny().await;
                return Err(());
            }
            Err(e) => {
                log_trace(trace_id, "quota_reserve", &format!("err={e} => fail"));
                let _ = delete_message(api, chat_id, message_id).await;
                std::fs::remove_dir_all(tmp_dir).ok();
                flow_manager.clear(user_id);
                crate::rank::paywall::quota_db_error(
                    api,
                    chat_id,
                    "separation",
                    &format!("{e}"),
                )
                .await;
                return Err(());
            }
        }

        if let Some(w_res) = weekly_res {
            match w_res {
                Ok(Some(used)) => {
                    reserved = true;
                    log_trace(
                        trace_id,
                        "quota_reserved_weekly",
                        &format!("used={used} limit={weekly_limit}"),
                    );
                }
                Ok(None) => {
                    handle_deny().await;
                    return Err(());
                }
                Err(e) => {
                    log_trace(trace_id, "quota_reserve", &format!("err={e} => fail"));
                    let _ = delete_message(api, chat_id, message_id).await;
                    std::fs::remove_dir_all(tmp_dir).ok();
                    flow_manager.clear(user_id);
                    crate::rank::paywall::quota_db_error(
                        api,
                        chat_id,
                        "separation",
                        &format!("{e}"),
                    )
                    .await;
                    return Err(());
                }
            }
        }
    }

    Ok(QuotaReservation {
        reserved,
        reserve_secs,
    })
}

pub async fn refund_quota(
    database: &Option<PostgresDatabase>,
    user_id: i64,
    reserve_secs: i64,
    reserved: bool,
    trace_id: u64,
    why: &str,
) {
    if reserved {
        if let Some(db) = database.as_ref() {
            log_trace(trace_id, "quota_refund", &format!("why={why}"));
            if let Ok(client) = db.get().await {
                for (kind, window) in [
                    (QuotaKind::SeparationDaily, 86400),
                    (QuotaKind::SeparationWeekly, 7 * 86400),
                ] {
                    if let Err(e) =
                        refund_usage(&client, user_id, kind, reserve_secs, window).await
                    {
                        log_trace(trace_id, "quota_refund", &format!("err={e} => fail"));
                        crate::stats::record_error_global(
                            "separation",
                            "quota_refund_failed",
                        )
                        .await;
                    }
                }
            }
        }
    }
}
