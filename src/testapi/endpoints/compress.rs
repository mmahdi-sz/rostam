use axum::Json;
use serde::{Deserialize, Serialize};
use crate::rank::types::Rank;

#[allow(dead_code)]
#[derive(Deserialize)]
pub struct CompressReq {
    pub user_id: Option<i64>,
    pub fmt: Option<String>,
    pub level: Option<u8>,
    pub algo: Option<String>,
    pub password: Option<String>,
    pub split_mb: Option<u32>,
    pub obfuscate: Option<bool>,
    pub solid: Option<bool>,
    pub file_count: Option<usize>,
}

#[derive(Serialize)]
pub struct CompressResp {
    pub ok: bool,
    pub fmt: String,
    pub level: u8,
    pub file_count: usize,
    pub welcome_text: String,
    pub result_caption: String,
    pub paywall_daily_limit_secs: u64,
    pub paywall_monthly_limit_secs: u64,
}

pub async fn test_filecompress(
    Json(req): Json<CompressReq>,
) -> (axum::http::StatusCode, Json<CompressResp>) {
    let fmt = req.fmt.unwrap_or_else(|| "7z".to_string());
    let level = req.level.unwrap_or(5);
    let file_count = req.file_count.unwrap_or(1);
    let rank = Rank::Dalavar;

    let welcome_text = crate::i18n::t("fc.welcome");
    let result_caption = crate::i18n::t("fc.result_caption");

    (
        axum::http::StatusCode::OK,
        Json(CompressResp {
            ok: true,
            fmt,
            level,
            file_count,
            welcome_text,
            result_caption,
            paywall_daily_limit_secs: rank.compress_cpu_daily_secs(),
            paywall_monthly_limit_secs: rank.compress_cpu_monthly_secs(),
        }),
    )
}
