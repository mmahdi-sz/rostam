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
