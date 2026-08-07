//! اندپوینت تست لایه‌ی سهمیه — همان توابع واقعی `rank::quota` روی دیتابیس dev.
//!
//! هندلرهای upscale/denoise خودشان بدون فایل رسانه و ONNX قابل اجرا نیستند، اما
//! چیزی که در plan 015 عوض شد دقیقاً همین گیت است: «چک + کسر» در یک statement.
//! این اندپوینت آن مسیر را بدون بازنویسی صدا می‌زند (rule #2)، پس هم مسیر
//! موفق و هم مسیر سهمیه‌تمام‌شده تست می‌شود.
//!
//! ponytail: بدون سیم‌کشی به هندلر کامل — نیاز به فایل واقعی تلگرام و مدل دارد؛
//! اگر روزی یک fixture رسانه‌ای اضافه شد، همان هندلر را صدا بزنید.

use axum::Json;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::rank::quota::{QuotaKind, get_usage, refund_usage, reserve_usage};

fn parse_kind(s: &str) -> Option<QuotaKind> {
    // ponytail: فقط انواعی که plan 015 لمس کرد؛ بقیه در صورت نیاز اضافه شود.
    Some(match s {
        "upscale_2x_weekly" => QuotaKind::Upscale2xWeekly,
        "upscale_3x_weekly" => QuotaKind::Upscale3xWeekly,
        "upscale_4x_weekly" => QuotaKind::Upscale4xWeekly,
        "deoldify_weekly" => QuotaKind::DeoldifyWeekly,
        "nobg_weekly" => QuotaKind::NobgWeekly,
        "tts_weekly" => QuotaKind::TtsWeekly,
        "denoise_daily" => QuotaKind::DenoiseDaily,
        "denoise_weekly" => QuotaKind::DenoiseWeekly,
        "separation_daily" => QuotaKind::SeparationDaily,
        "separation_weekly" => QuotaKind::SeparationWeekly,
        "compress_cpu_daily" => QuotaKind::CompressCpuDaily,
        "compress_cpu_monthly" => QuotaKind::CompressCpuMonthly,
        _ => return None,
    })
}

#[derive(Deserialize)]
pub struct QuotaReq {
    pub user_id: i64,
    pub kind: String,
    /// `reserve` | `refund` | `get`
    pub action: String,
    #[serde(default)]
    pub amount: i64,
    pub window_secs: i64,
    #[serde(default)]
    pub limit: i64,
}

#[derive(Serialize)]
pub struct QuotaResp {
    pub ok: bool,
    pub action: String,
    pub kind: String,
    /// در `reserve`: مصرف بعد از رزرو. `null` یعنی رد شد (سقف).
    pub used_after: Option<i64>,
    /// در `get`/`refund`: مصرف فعلی.
    pub used: Option<i64>,
    pub granted: bool,
    pub error: Option<String>,
}

pub async fn test_quota(Json(req): Json<QuotaReq>) -> axum::response::Response {
    let Some(kind) = parse_kind(&req.kind) else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            format!("unknown quota kind: {}", req.kind),
        )
            .into_response();
    };

    let Some(db) = crate::testapi::state::db().await.as_ref() else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "db unavailable",
        )
            .into_response();
    };
    let client = db.client();

    let mut resp = QuotaResp {
        ok: true,
        action: req.action.clone(),
        kind: req.kind.clone(),
        used_after: None,
        used: None,
        granted: false,
        error: None,
    };

    match req.action.as_str() {
        "reserve" => {
            match reserve_usage(
                client,
                req.user_id,
                kind,
                req.amount,
                req.window_secs,
                req.limit,
            )
            .await
            {
                Ok(Some(used)) => {
                    resp.granted = true;
                    resp.used_after = Some(used);
                }
                Ok(None) => resp.granted = false,
                Err(e) => {
                    resp.ok = false;
                    resp.error = Some(format!("{e}"));
                }
            }
        }
        "refund" => {
            if let Err(e) =
                refund_usage(client, req.user_id, kind, req.amount, req.window_secs).await
            {
                resp.ok = false;
                resp.error = Some(format!("{e}"));
            }
        }
        "get" => {}
        other => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                format!("unknown action: {other}"),
            )
                .into_response();
        }
    }

    resp.used = get_usage(client, req.user_id, kind, req.window_secs)
        .await
        .ok();

    Json(resp).into_response()
}
