use axum::Json;
use serde::{Deserialize, Serialize};

/// Test user ID for spending points in `-999xxx` range.
const SPEND_UID: i64 = -999_201;

#[derive(Deserialize)]
pub struct ReferralSpendReq {
    /// Number of verified referrals seeded for test user.
    pub points: i32,
    pub tier: String,
}

#[derive(Serialize)]
pub struct ReferralSpendResp {
    pub ok: bool,
    pub tier: String,
    pub required_points: i32,
    pub remaining_points: i32,
    pub days_added: i32,
    /// Real handler output message.
    pub message: String,
    /// Granted rank stored in `user_ranks` (`None` if unchanged).
    pub granted_rank: Option<String>,
    pub db: String,
}

/// Tests referral point spending via `rank::panel::process_claim`.
pub async fn test_referral_spend(Json(req): Json<ReferralSpendReq>) -> Json<ReferralSpendResp> {
    // Normalize tier string casing.
    let tier_key = req.tier.to_ascii_lowercase();
    let Some(&(required, tier_rank)) = crate::referral::TIERS
        .iter()
        .find(|(_, r)| r.as_str() == tier_key)
    else {
        return Json(ReferralSpendResp {
            ok: false,
            tier: req.tier,
            required_points: 0,
            remaining_points: req.points,
            days_added: 0,
            message: "unknown tier".to_string(),
            granted_rank: None,
            db: "n/a".to_string(),
        });
    };

    let database = crate::testapi::state::db().await;
    let Some(db) = database else {
        return Json(ReferralSpendResp {
            ok: false,
            tier: req.tier,
            required_points: required as i32,
            remaining_points: req.points,
            days_added: 0,
            message: "database unavailable".to_string(),
            granted_rank: None,
            db: "unavailable".to_string(),
        });
    };
    let client = match db.get().await {
        Ok(c) => c,
        Err(e) => {
            return Json(ReferralSpendResp {
                ok: false,
                tier: req.tier,
                required_points: required as i32,
                remaining_points: req.points,
                days_added: 0,
                message: format!("db checkout failed: {e}"),
                granted_rank: None,
                db: "unavailable".to_string(),
            });
        }
    };

    // Seed referrals following standard sweep structure.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let _ = purge_spend(&client, SPEND_UID).await;
    if req.points > 0 {
        let n = req.points as i64;
        if let Err(e) = client
            .execute(
                "INSERT INTO referrals (referred_id, referrer_id, created_at)
                 SELECT $1::bigint - g, $1::bigint, $2::bigint FROM generate_series(1, $3::bigint) g",
                &[&SPEND_UID, &now, &n],
            )
            .await
        {
            return Json(ReferralSpendResp {
                ok: false,
                tier: req.tier,
                required_points: required as i32,
                remaining_points: req.points,
                days_added: 0,
                message: format!("seed failed: {e}"),
                granted_rank: None,
                db: "connected".to_string(),
            });
        }
    }

    let trace = crate::log::next_trace_id();
    let message = crate::rank::panel::process_claim(database, SPEND_UID, required, trace).await;

    let spent = crate::referral::total_spent_points(&client, SPEND_UID).await;
    let total = crate::referral::count_referrals(&client, SPEND_UID).await;
    let rank_row = crate::rank::store::get_user_rank(&client, SPEND_UID)
        .await
        .ok()
        .flatten();
    let granted_rank = rank_row.as_ref().map(|r| r.rank.as_str().to_string());
    let days_added = rank_row
        .as_ref()
        .and_then(|r| r.expires_at)
        .map(|exp| crate::rank::types::ceil_div((exp - now).max(0), 86_400) as i32)
        .unwrap_or(0);

    let _ = purge_spend(&client, SPEND_UID).await;

    Json(ReferralSpendResp {
        // Success: rank stored and points deducted.
        ok: granted_rank.as_deref() == Some(tier_rank.as_str()) && spent == required as i64,
        tier: req.tier,
        required_points: required as i32,
        remaining_points: (total - spent).max(0) as i32,
        days_added,
        message,
        granted_rank,
        db: "connected".to_string(),
    })
}

async fn purge_spend(
    client: &tokio_postgres::Client,
    user_id: i64,
) -> Result<(), tokio_postgres::Error> {
    client
        .execute(
            "DELETE FROM referral_activations WHERE user_id = $1",
            &[&user_id],
        )
        .await?;
    client
        .execute("DELETE FROM referrals WHERE referrer_id = $1", &[&user_id])
        .await?;
    client
        .execute("DELETE FROM user_ranks WHERE user_id = $1", &[&user_id])
        .await?;
    client
        .execute("DELETE FROM stats_events WHERE user_id = $1", &[&user_id])
        .await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct ReferralLeaderboardReq {
    pub sample_users: Option<Vec<crate::referral::TopReferrer>>,
}

#[derive(Serialize)]
pub struct ReferralLeaderboardResp {
    pub ok: bool,
    pub rendered_text: String,
    pub has_rlm: bool,
    pub inline_keyboard: serde_json::Value,
    pub stats_events: Vec<serde_json::Value>,
}

pub async fn test_referral_leaderboard(
    Json(req): Json<ReferralLeaderboardReq>,
) -> Json<ReferralLeaderboardResp> {
    let top_users = req.sample_users.unwrap_or_else(|| {
        vec![
            crate::referral::TopReferrer {
                user_id: 1001,
                username: Some("mmahdi_sz".to_string()),
                referral_count: 25,
            },
            crate::referral::TopReferrer {
                user_id: 1002,
                username: Some("username".to_string()),
                referral_count: 20,
            },
        ]
    });

    let text_raw = crate::referral::render_leaderboard_text(&top_users);
    let rendered_text = crate::i18n::apply_premium_to_md(&text_raw);
    let has_rlm = rendered_text.contains('\u{200F}');

    let back_kbd = crate::bot::keyboards::back_keyboard();
    let inline_keyboard = serde_json::to_value(back_kbd.inline_keyboard).unwrap_or_default();

    Json(ReferralLeaderboardResp {
        ok: true,
        rendered_text,
        has_rlm,
        inline_keyboard,
        stats_events: vec![serde_json::json!({
            "feature": "referral",
            "action": "leaderboard_view",
            "status": "ok",
            "amount": 1
        })],
    })
}
