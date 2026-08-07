//! `/test/redeem/apply` — مسیر واقعی مصرف کد هدیه.
//!
//! هیچ چیزی بازنویسی نشده: کد آزمایشی با همان `store::create_code` ساخته
//! می‌شود و تصمیم با همان `handle::handle_redeem` گرفته می‌شود که deep-link
//! واقعی صدا می‌زند. سه سناریو با همین یک اندپوینت پوشش داده می‌شود:
//! ۱) seed + مصرف موفق (`Consumed`)، ۲) صدا زدن دوباره با همان کاربر
//! (`AlreadyRedeemed`)، ۳) کد ناموجود (`redeem.invalid`).

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

/// شناسه‌های آزمایشی در بازه‌ی `-999xxx` می‌مانند تا با کاربر واقعی برخورد نکنند.
const DEFAULT_UID: i64 = -999_200;
const DEFAULT_CODE: &str = "TESTAPIREDEEM";

#[derive(Deserialize)]
pub struct RedeemApplyReq {
    pub code: Option<String>,
    pub user_id: Option<i64>,
    /// قبل از اجرا کد را از نو بساز (و ردیف‌های قبلی‌اش را پاک کن).
    pub seed: Option<bool>,
    pub max_uses: Option<i32>,
    pub rank: Option<String>,
    pub duration_days: Option<i32>,
    /// بعد از اجرا ردیف‌های آزمایشی پاک شوند (پیش‌فرض: نه، تا تماس دوم
    /// بتواند `AlreadyRedeemed` را ببیند).
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
    let db_status = if database.is_some() {
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
        let client = db.client();
        // شروع تمیز: اجرای دوباره‌ی سوئیت نباید به ردیف اجرای قبلی گیر کند.
        if let Err(e) = purge(client, &code, user_id).await {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("seed cleanup failed: {e}"),
            )
                .into_response();
        }
        if let Err(e) = crate::redeem::store::create_code(
            client,
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

    let api = Bot::new_url(format!(
        "http://127.0.0.1:{}/bot",
        std::env::var("TESTAPI_PORT").unwrap_or_else(|_| "14379".to_string())
    ));

    let traces = Arc::new(Mutex::new(Vec::new()));
    let stats = Arc::new(Mutex::new(Vec::new()));
    let i18n_keys = Arc::new(Mutex::new(Vec::new()));
    let emojis = Arc::new(Mutex::new(Vec::new()));

    let ok = CAPTURED_TRACES
        .scope(traces.clone(), async {
            CAPTURED_STATS
                .scope(stats.clone(), async {
                    RESOLVED_I18N_KEYS
                        .scope(i18n_keys.clone(), async {
                            CAPTURED_EMOJIS
                                .scope(emojis.clone(), async {
                                    // هندلر واقعیِ deep-link — بدون هیچ shim.
                                    crate::redeem::handle::handle_redeem(
                                        &api,
                                        user_id,
                                        user_id,
                                        "TestApi",
                                        Some("testapi"),
                                        &code,
                                        database,
                                    )
                                    .await
                                })
                                .await
                        })
                        .await
                })
                .await
        })
        .await;

    // وضعیت واقعی دیتابیس بعد از اجرا — همان چیزی که باگ «دو ظرفیت سوخته» را
    // لو می‌دهد، نه متن پیام.
    let (used_count, redemption_rows, user_rank) = match database {
        Some(db) => {
            let client = db.client();
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
            let rank = crate::rank::store::get_user_rank(client, user_id)
                .await
                .ok()
                .flatten()
                .map(|r| json!({ "rank": r.rank.as_str(), "expires_at": r.expires_at }));
            (used, rows, rank)
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
            if let Err(e) = purge(db.client(), &code, user_id).await {
                eprintln!("[testapi] redeem cleanup failed: {e}");
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

/// پاک‌کردن هر اثری از این کد/کاربر آزمایشی. فقط ردیف‌های `-999xxx` و کد
/// آزمایشی را می‌بیند، پس روی داده‌ی واقعی dev اثری ندارد.
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
