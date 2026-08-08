use crate::filecompress::CompressConfig;
use crate::filecompress::handle as fc;
use crate::rank::types::Rank;
use axum::Json;
use serde::{Deserialize, Serialize};

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

// ── UX surface (کیبورد/توست/تیکر) ───────────────────────────────────────────────

#[derive(Deserialize)]
pub struct FcUxReq {
    pub fmt: Option<String>,
    pub level: Option<u8>,
    pub solid: Option<bool>,
    /// درجه‌ی درخواستی برای «+»؛ برای بازتولید توست حداکثر فشرده‌سازی.
    pub bump_level: Option<bool>,
}

#[derive(Serialize)]
pub struct FcButton {
    pub text: String,
    pub callback_data: String,
    pub custom_emoji_id: Option<String>,
    /// رنگ دکمه: success (سبز) / primary (آبی) / danger (قرمز).
    pub style: String,
}

#[derive(Serialize)]
pub struct FcUxResp {
    pub ok: bool,
    pub fmt: String,
    pub level: u8,
    pub max_level: u8,
    pub solid: bool,
    /// آیا فرمت انتخابی این دکمه‌ها را دارد؟ zstd هیچ‌کدام را ندارد و باید
    /// از کیبورد حذف شده باشد، نه اینکه غیرفعال نمایش داده شود.
    pub has_password_button: bool,
    pub has_split_button: bool,
    pub has_solid_button: bool,
    /// متن معرفی همان فرمت (welcome / welcome_7z / welcome_zstd) پس از i18n.
    pub welcome_text: String,
    /// دکمهٔ حالت فشرده‌سازی: باید در solid آبی و در حالت عادی سبز باشد.
    pub solid_button: FcButton,
    pub solid_button_color: String,
    /// توست حداکثر فشرده‌سازی (خالی اگر روی سقف نباشیم).
    pub max_level_toast: Option<String>,
    pub options_keyboard: Vec<Vec<FcButton>>,
    pub ask_password_text: String,
    pub ask_password_keyboard: Vec<Vec<FcButton>>,
    pub password_need_text: String,
    pub progress_text: String,
    pub progress_keyboard: Vec<Vec<FcButton>>,
    pub cancelled_text: String,
    pub reenter_prompt: String,
}

fn dump(kbd: &frankenstein::types::InlineKeyboardMarkup) -> Vec<Vec<FcButton>> {
    kbd.inline_keyboard
        .iter()
        .map(|row| {
            row.iter()
                .map(|b| FcButton {
                    text: b.text.clone(),
                    callback_data: b.callback_data.clone().unwrap_or_default(),
                    custom_emoji_id: b.icon_custom_emoji_id.clone(),
                    style: match b.style {
                        Some(frankenstein::types::ButtonStyle::Success) => "success",
                        Some(frankenstein::types::ButtonStyle::Primary) => "primary",
                        Some(frankenstein::types::ButtonStyle::Danger) => "danger",
                        _ => "default",
                    }
                    .to_string(),
                })
                .collect()
        })
        .collect()
}

pub async fn test_filecompress_ux(
    Json(req): Json<FcUxReq>,
) -> (axum::http::StatusCode, Json<FcUxResp>) {
    let mut config = CompressConfig::default();
    if let Some(f) = req.fmt.as_deref() {
        if let Some(parsed) = crate::filecompress::CompressFmt::from_str(f) {
            config.fmt = parsed;
        }
    }
    let max_level: u8 = config.fmt.max_level();
    config.level = req.level.unwrap_or(config.level).min(max_level);
    config.solid = req.solid.unwrap_or(config.solid);

    if req.bump_level.unwrap_or(false) && config.level < max_level {
        config.level += 1;
    }

    let max_level_toast = if config.level >= max_level {
        Some(crate::i18n::tf(
            "fc.max_level_notice",
            &[
                ("fmt", config.fmt.as_str()),
                ("max", &max_level.to_string()),
            ],
        ))
    } else {
        None
    };

    let kbd = fc::options_keyboard_for_test(&config);
    let rows = dump(&kbd);
    // ردیف حالت فشرده‌سازی: دکمهٔ دوم همان سوییچ رنگی است.
    let solid_button = rows
        .iter()
        .flatten()
        .filter(|b| b.callback_data == "fc:toggle:solid")
        .last()
        .map(|b| FcButton {
            text: b.text.clone(),
            callback_data: b.callback_data.clone(),
            custom_emoji_id: b.custom_emoji_id.clone(),
            style: b.style.clone(),
        })
        .unwrap_or(FcButton {
            text: String::new(),
            callback_data: String::new(),
            custom_emoji_id: None,
            style: "default".to_string(),
        });
    let solid_button_color = solid_button.style.clone();

    let has_button = |cb: &str| rows.iter().flatten().any(|b| b.callback_data == cb);
    let has_password_button = has_button("fc:toggle:pass");
    let has_split_button = has_button("fc:toggle:split");
    let has_solid_button = has_button("fc:toggle:solid");
    let welcome_text = crate::i18n::t(match config.fmt {
        crate::filecompress::CompressFmt::SevenZ => "fc.welcome_7z",
        crate::filecompress::CompressFmt::Zstd => "fc.welcome_zstd",
        _ => "fc.welcome",
    });

    let progress_text = crate::i18n::t("fc.processing")
        .replace("{bar}", "░░░░░░░░░░")
        .replace("{percent}", "0")
        .replace("{elapsed}", &fc::format_clock_for_test(125));

    (
        axum::http::StatusCode::OK,
        Json(FcUxResp {
            ok: true,
            fmt: config.fmt.as_str().to_string(),
            level: config.level,
            max_level,
            solid: config.solid,
            has_password_button,
            has_split_button,
            has_solid_button,
            welcome_text,
            solid_button,
            solid_button_color,
            max_level_toast,
            options_keyboard: rows,
            ask_password_text: crate::i18n::t("fc.ask_password"),
            ask_password_keyboard: dump(&fc::cancel_only_keyboard_for_test()),
            password_need_text: crate::i18n::t("fc.password_need_text"),
            progress_text,
            progress_keyboard: dump(&fc::job_cancel_keyboard_for_test()),
            cancelled_text: crate::i18n::t("fc.cancelled"),
            reenter_prompt: crate::i18n::t("fc.upload_prompt").replace("{count}", "0"),
        }),
    )
}
