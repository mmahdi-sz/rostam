use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct GuideReq {
    /// Empty/absent = the guide menu itself; otherwise `yt` | `sp` | `sc`.
    #[serde(default)]
    pub platform: String,
}

#[derive(Serialize)]
pub struct GuideResp {
    pub ok: bool,
    pub platform: String,
    pub known_platform: bool,
    pub i18n_keys: Vec<String>,
    pub rendered_text: String,
    /// UTF-16 units, as Telegram counts them (cap 4096).
    pub text_len_utf16: usize,
    pub within_telegram_cap: bool,
    pub button_labels: Vec<String>,
    pub button_callbacks: Vec<String>,
    pub button_emoji_ids: Vec<Option<String>>,
    pub start_menu_first_row_callback: String,
    pub mentions_autodetect: bool,
}

/// Renders the social-media guide menu, or one platform page, through the real
/// `bot::keyboards` functions. Unknown platform = the failure path the dispatcher
/// rejects, so it returns `ok=false` with the menu untouched.
pub async fn test_start_guide(Json(req): Json<GuideReq>) -> Json<GuideResp> {
    let known_platform = crate::bot::keyboards::GUIDE_PLATFORMS.contains(&req.platform.as_str());
    let is_menu = req.platform.is_empty();

    let (rendered_text, kb, i18n_keys) = if is_menu {
        (
            crate::i18n::apply_premium_to_md(&crate::i18n::t("start.guide_title")),
            crate::bot::keyboards::guide_keyboard(),
            vec!["start.guide_title".to_string()],
        )
    } else if known_platform {
        (
            crate::bot::keyboards::guide_platform_text(&req.platform),
            crate::bot::keyboards::guide_platform_keyboard(),
            vec![
                format!("start.guide_{}_text", req.platform),
                "start.guide_autodetect".to_string(),
            ],
        )
    } else {
        (
            String::new(),
            crate::bot::keyboards::guide_keyboard(),
            Vec::new(),
        )
    };

    let mut button_labels = Vec::new();
    let mut button_callbacks = Vec::new();
    let mut button_emoji_ids = Vec::new();
    for row in &kb.inline_keyboard {
        for btn in row {
            button_labels.push(btn.text.clone());
            button_callbacks.push(btn.callback_data.clone().unwrap_or_default());
            button_emoji_ids.push(btn.icon_custom_emoji_id.clone());
        }
    }

    let len = rendered_text.encode_utf16().count();
    let start_kb = crate::bot::keyboards::start_menu_keyboard(false);
    let first_row_cb = start_kb
        .inline_keyboard
        .first()
        .and_then(|r| r.first())
        .and_then(|b| b.callback_data.clone())
        .unwrap_or_default();

    Json(GuideResp {
        ok: is_menu || known_platform,
        known_platform,
        i18n_keys,
        // Compare against the rendered form: the note's 💡 becomes a custom-emoji
        // span in `rendered_text`, so the raw i18n line never matches.
        mentions_autodetect: !is_menu
            && known_platform
            && rendered_text.contains(
                crate::i18n::apply_premium_to_md(&crate::i18n::t("start.guide_autodetect"))
                    .lines()
                    .next_back()
                    .unwrap_or_default(),
            ),
        text_len_utf16: len,
        within_telegram_cap: len <= 4096,
        rendered_text,
        button_labels,
        button_callbacks,
        button_emoji_ids,
        start_menu_first_row_callback: first_row_cb,
        platform: req.platform,
    })
}
