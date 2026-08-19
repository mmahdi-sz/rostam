//! `/test/redeem/apply` — test endpoint for code redemption.

use crate::bot::messaging::CAPTURED_EMOJIS;
use crate::i18n::RESOLVED_I18N_KEYS;
use crate::log::CAPTURED_TRACES;
use crate::rank::types::Rank;
use crate::stats::CAPTURED_STATS;
use crate::testapi::state::clear_payloads;
use axum::Json;
use axum::response::IntoResponse;
use frankenstein::client_reqwest::Bot;
use serde::Deserialize;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// Test user ID in `-999xxx` range to avoid collisions.
const DEFAULT_UID: i64 = -999_200;
const DEFAULT_CODE: &str = "TESTAPIREDEEM";

#[derive(Deserialize)]
pub struct RedeemApplyReq {
    pub code: Option<String>,
    pub user_id: Option<i64>,
    /// Re-seed code before execution (clears prior rows).
    pub seed: Option<bool>,
    pub max_uses: Option<i32>,
    pub rank: Option<String>,
    pub duration_days: Option<i32>,
    /// Clean up test rows after execution (default false).
    pub cleanup: Option<bool>,
}

pub async fn test_redeem_apply(Json(req): Json<RedeemApplyReq>) -> axum::response::Response {
    clear_payloads();

    let code = req.code.unwrap_or_else(|| DEFAULT_CODE.to_string());
    let user_id = req.user_id.unwrap_or(DEFAULT_UID);
    let max_uses = req.max_uses.unwrap_or(1);
    let duration_days = req.duration_days.unwrap_or(7);
    let rank_str = req.rank.unwrap_or_else(|| "Sepahbod".to_string());
    let Some(rank) = Rank::from_str(&rank_str.to_ascii_lowercase()) else {
        return (axum::http::StatusCode::BAD_REQUEST, "unknown rank").into_response();
    };

    let database = crate::testapi::state::db().await;
    let _db_status = if database.is_some() {
        "connected"
    } else {
        "unavailable"
    };

    if req.seed.unwrap_or(false) {
        let Some(db) = database else {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "seed requested but DATABASE_URL is not reachable",
            )
                .into_response();
        };
        let client = match db.get().await {
            Ok(c) => c,
            Err(e) => {
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    format!("db checkout failed: {e}"),
                )
                    .into_response();
            }
        };
        // Clean start for seed run.
        if let Err(e) = purge(&client, &code, user_id).await {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("seed cleanup failed: {e}"),
            )
                .into_response();
        }
        if let Err(e) = crate::redeem::store::create_code(
            &client,
            &code,
            rank,
            duration_days,
            max_uses,
            DEFAULT_UID,
        )
        .await
        {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("seed create_code failed: {e}"),
            )
                .into_response();
        }
    }

    let traces = Arc::new(Mutex::new(Vec::new()));
    let stats = Arc::new(Mutex::new(Vec::new()));
    let i18n_keys = Arc::new(Mutex::new(Vec::new()));
    let emojis = Arc::new(Mutex::new(Vec::new()));

    let bot = Bot::new("123456:TESTAPI_DUMMY_TOKEN");

    let (ok, db_status) = CAPTURED_TRACES
        .scope(traces.clone(), {
            let stats = stats.clone();
            let i18n_keys = i18n_keys.clone();
            let emojis = emojis.clone();
            CAPTURED_STATS
                .scope(stats, {
                    RESOLVED_I18N_KEYS
                        .scope(i18n_keys, {
                            CAPTURED_EMOJIS
                                .scope(emojis, async {
                                    match database {
                                        Some(db) => {
                                            let db_opt = Some(db.clone());
                                            let res = crate::redeem::handle::handle_redeem(
                                                &bot,
                                                user_id,
                                                user_id,
                                                "Test",
                                                None,
                                                &code,
                                                &db_opt,
                                            )
                                            .await;
                                            (res, "connected".to_string())
                                        }
                                        None => {
                                            let res = crate::redeem::handle::handle_redeem(
                                                &bot,
                                                user_id,
                                                user_id,
                                                "Test",
                                                None,
                                                &code,
                                                &None,
                                            )
                                            .await;
                                            (res, "unavailable".to_string())
                                        }
                                    }
                                })
                        })
                })
        })
        .await;

    // Fetch database state post-execution.
    let (used_count, redemption_rows, user_rank) = match database {
        Some(db) => {
            if let Ok(client) = db.get().await {
                let used = client
                    .query_opt(
                        "SELECT used_count FROM redeem_codes WHERE code = $1",
                        &[&code],
                    )
                    .await
                    .ok()
                    .flatten()
                    .map(|r| r.get::<_, i32>(0));
                let rows: i64 = client
                    .query_one(
                        "SELECT count(*) FROM redeem_redemptions WHERE code = $1",
                        &[&code],
                    )
                    .await
                    .map(|r| r.get(0))
                    .unwrap_or(-1);
                let rank = crate::rank::store::get_user_rank(&client, user_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|r| json!({ "rank": r.rank.as_str(), "expires_at": r.expires_at }));
                (used, rows, rank)
            } else {
                (None, -1, None)
            }
        }
        None => (None, -1, None),
    };

    let payloads = crate::testapi::state::CAPTURED_PAYLOADS
        .lock()
        .unwrap()
        .clone();
    let rendered_text = payloads
        .iter()
        .find_map(|p| p.get("text").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_default();

    if req.cleanup.unwrap_or(false) {
        if let Some(db) = database {
            if let Ok(client) = db.get().await {
                if let Err(e) = purge(&client, &code, user_id).await {
                    eprintln!("[testapi] redeem cleanup failed: {e}");
                }
            }
        }
    }

    Json(json!({
        "ok": ok,
        "db": db_status,
        "code": code,
        "user_id": user_id,
        "rendered_text": rendered_text,
        "used_count": used_count,
        "redemption_rows": redemption_rows,
        "user_rank": user_rank,
        "payloads": payloads,
        "traces": traces.lock().unwrap().clone(),
        "stats_events": stats.lock().unwrap().clone(),
        "i18n_keys": i18n_keys.lock().unwrap().clone(),
        "custom_emojis": emojis.lock().unwrap().clone(),
    }))
    .into_response()
}

/// Purges test code and user records (`-999xxx`).
async fn purge(
    client: &tokio_postgres::Client,
    code: &str,
    user_id: i64,
) -> Result<(), tokio_postgres::Error> {
    client
        .execute("DELETE FROM redeem_redemptions WHERE code = $1", &[&code])
        .await?;
    client
        .execute("DELETE FROM redeem_codes WHERE code = $1", &[&code])
        .await?;
    client
        .execute("DELETE FROM user_ranks WHERE user_id = $1", &[&user_id])
        .await?;
    client
        .execute("DELETE FROM stats_events WHERE user_id = $1", &[&user_id])
        .await?;
    Ok(())
}
