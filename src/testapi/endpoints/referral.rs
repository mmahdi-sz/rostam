use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct ReferralSpendReq {
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
}

pub async fn test_referral_spend(Json(req): Json<ReferralSpendReq>) -> Json<ReferralSpendResp> {
    let (required, days) = match req.tier.as_str() {
        "Sohrab" => (10, 31),
        "Esfandyar" => (20, 31),
        "Rostam" => (50, 31),
        _ => (10, 31),
    };
    let ok = req.points >= required;
    Json(ReferralSpendResp {
        ok,
        tier: req.tier,
        required_points: required,
        remaining_points: if ok {
            req.points - required
        } else {
            req.points
        },
        days_added: if ok { days } else { 0 },
    })
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
    let top_users = req.sample_users.unwrap_or_else(|| vec![
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
    ]);

    let text_raw = crate::referral::render_leaderboard_text(&top_users);
    let rendered_text = crate::i18n::apply_premium_to_md(&text_raw);
    let has_rlm = rendered_text.contains('\u{200F}');

    let back_kbd = crate::bot::keyboards::back_keyboard();
    let inline_keyboard = serde_json::to_value(back_kbd.inline_keyboard)
        .unwrap_or_default();

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

