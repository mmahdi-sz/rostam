//! TestAPI endpoints for the album/playlist (musicset) feature.

use axum::Json;
use serde::{Deserialize, Serialize};

use crate::musicset::{
    CB_MS_CANCEL, CB_MS_JOBCANCEL, CB_MS_MODE_ONE, CB_MS_MODE_ZIP, SetItems, SetSource, detect_set,
    fetch_set, mode_keyboard,
};
use crate::rank::types::Rank;

#[derive(Deserialize)]
pub struct MsOfferReq {
    pub url: String,
    /// رنک شبیه‌سازی‌شده: dalavar | sepahbod | esfandyar | sohrab | rostam
    pub rank: Option<String>,
    /// `true` = فهرست واقعی ترک‌ها هم از پلتفرم گرفته شود
    pub fetch: Option<bool>,
}

#[derive(Serialize)]
pub struct MsButton {
    pub text: String,
    pub callback_data: String,
    pub icon_custom_emoji_id: Option<String>,
}

#[derive(Serialize)]
pub struct MsOfferResp {
    pub ok: bool,
    pub detected: Option<String>,
    pub platform: Option<String>,
    pub rank: String,
    pub blocked: bool,
    pub paywall_min_rank: Option<String>,
    pub track_limit: Option<u32>,
    pub can_archive: bool,
    pub set_title: Option<String>,
    pub tracks_total: Option<usize>,
    pub tracks_queued: Option<usize>,
    pub status_text: String,
    pub i18n_keys: Vec<String>,
    pub keyboard: Vec<Vec<MsButton>>,
    pub trace: u64,
}

fn rank_of(name: &Option<String>) -> Rank {
    name.as_deref()
        .and_then(Rank::from_str)
        .unwrap_or(Rank::Dalavar)
}

fn keyboard_dump() -> Vec<Vec<MsButton>> {
    mode_keyboard()
        .inline_keyboard
        .iter()
        .map(|row| {
            row.iter()
                .map(|b| MsButton {
                    text: b.text.clone(),
                    callback_data: b.callback_data.clone().unwrap_or_default(),
                    icon_custom_emoji_id: b.icon_custom_emoji_id.clone(),
                })
                .collect()
        })
        .collect()
}

pub async fn test_ms_offer(Json(req): Json<MsOfferReq>) -> Json<MsOfferResp> {
    let trace = crate::log::next_trace_id();
    let rank = rank_of(&req.rank);
    let limit = rank.music_set_limit();
    let source = detect_set(&req.url);

    let (detected, platform) = match &source {
        Some(SetSource::Spotify(kind, id)) => (Some(id.clone()), Some(kind.as_str().to_string())),
        Some(SetSource::Soundcloud(url)) => (Some(url.clone()), Some("soundcloud".to_string())),
        None => (None, None),
    };

    // مسیر خطا: لینک ست نیست
    let Some(source) = source else {
        return Json(MsOfferResp {
            ok: false,
            detected,
            platform,
            rank: rank.as_str().to_string(),
            blocked: false,
            paywall_min_rank: None,
            track_limit: limit,
            can_archive: rank.can_music_set_archive(),
            set_title: None,
            tracks_total: None,
            tracks_queued: None,
            status_text: crate::i18n::t("musicset.list_failed"),
            i18n_keys: vec!["musicset.list_failed".to_string()],
            keyboard: vec![],
            trace,
        });
    };

    // مسیر paywall: دلاور/سهراب کلاً ممنوع
    if limit == Some(0) {
        return Json(MsOfferResp {
            ok: false,
            detected,
            platform,
            rank: rank.as_str().to_string(),
            blocked: true,
            paywall_min_rank: Some(Rank::Esfandyar.as_str().to_string()),
            track_limit: limit,
            can_archive: false,
            set_title: None,
            tracks_total: None,
            tracks_queued: None,
            status_text: crate::i18n::tf(
                "rank.paywall_feature",
                &[
                    ("feature", &crate::i18n::t("musicset.feature_name")),
                    ("min_rank", &Rank::Esfandyar.display_name()),
                ],
            ),
            i18n_keys: vec![
                "musicset.feature_name".to_string(),
                "rank.paywall_feature".to_string(),
            ],
            keyboard: vec![],
            trace,
        });
    }

    let mut set_title = None;
    let mut tracks_total = None;
    let mut tracks_queued = None;
    let mut status_text = crate::i18n::t("musicset.fetching_list");
    let mut i18n_keys = vec!["musicset.fetching_list".to_string()];

    if req.fetch.unwrap_or(false) {
        match fetch_set(trace, &source).await {
            Ok(set) => {
                let total = set.len();
                let queued = match limit {
                    Some(max) => total.min(max as usize),
                    None => total,
                };
                let kind = match &set.items {
                    SetItems::Spotify(_) => "spotify",
                    SetItems::Soundcloud(_) => "soundcloud",
                };
                status_text = crate::i18n::tf(
                    "musicset.ask_mode",
                    &[
                        ("title", &crate::i18n::md_escape(&set.title)),
                        ("count", &queued.to_string()),
                    ],
                );
                i18n_keys = vec![
                    "musicset.ask_mode".to_string(),
                    "musicset.btn_one".to_string(),
                    "musicset.btn_zip".to_string(),
                    "musicset.btn_cancel".to_string(),
                ];
                if queued < total {
                    status_text.push('\n');
                    status_text.push_str(&crate::i18n::tf(
                        "musicset.capped_notice",
                        &[
                            ("shown", &queued.to_string()),
                            ("total", &total.to_string()),
                        ],
                    ));
                    i18n_keys.push("musicset.capped_notice".to_string());
                }
                set_title = Some(set.title);
                tracks_total = Some(total);
                tracks_queued = Some(queued);
                let _ = kind;
            }
            Err(e) => {
                return Json(MsOfferResp {
                    ok: false,
                    detected,
                    platform,
                    rank: rank.as_str().to_string(),
                    blocked: false,
                    paywall_min_rank: None,
                    track_limit: limit,
                    can_archive: rank.can_music_set_archive(),
                    set_title: None,
                    tracks_total: None,
                    tracks_queued: None,
                    status_text: format!("{} ({e})", crate::i18n::t("musicset.list_failed")),
                    i18n_keys: vec!["musicset.list_failed".to_string()],
                    keyboard: vec![],
                    trace,
                });
            }
        }
    }

    Json(MsOfferResp {
        ok: true,
        detected,
        platform,
        rank: rank.as_str().to_string(),
        blocked: false,
        paywall_min_rank: None,
        track_limit: limit,
        can_archive: rank.can_music_set_archive(),
        set_title,
        tracks_total,
        tracks_queued,
        status_text,
        i18n_keys,
        keyboard: keyboard_dump(),
        trace,
    })
}

#[derive(Deserialize)]
pub struct MsModeReq {
    pub rank: Option<String>,
    pub zip: bool,
}

#[derive(Serialize)]
pub struct MsModeResp {
    pub ok: bool,
    pub rank: String,
    pub mode: String,
    pub blocked: bool,
    pub paywall_min_rank: Option<String>,
    pub status_text: String,
    pub i18n_keys: Vec<String>,
    pub mode_callback: String,
    pub cancel_callback: String,
    pub job_cancel_callback: String,
    pub split_mb: u32,
    pub archive_level: u8,
}

pub async fn test_ms_mode(Json(req): Json<MsModeReq>) -> Json<MsModeResp> {
    let rank = rank_of(&req.rank);
    let blocked = req.zip && !rank.can_music_set_archive();

    let (status_text, i18n_keys) = if blocked {
        (
            crate::i18n::tf(
                "rank.paywall_limit",
                &[
                    ("limit", &crate::i18n::t("musicset.zip_limit")),
                    ("min_rank", &Rank::Esfandyar.display_name()),
                ],
            ),
            vec![
                "musicset.zip_limit".to_string(),
                "rank.paywall_limit".to_string(),
            ],
        )
    } else {
        (
            crate::i18n::t("musicset.starting"),
            vec!["musicset.starting".to_string()],
        )
    };

    Json(MsModeResp {
        ok: !blocked,
        rank: rank.as_str().to_string(),
        mode: if req.zip { "zip" } else { "one" }.to_string(),
        blocked,
        paywall_min_rank: blocked.then(|| Rank::Esfandyar.as_str().to_string()),
        status_text,
        i18n_keys,
        mode_callback: if req.zip {
            CB_MS_MODE_ZIP
        } else {
            CB_MS_MODE_ONE
        }
        .to_string(),
        cancel_callback: CB_MS_CANCEL.to_string(),
        job_cancel_callback: CB_MS_JOBCANCEL.to_string(),
        split_mb: crate::musicset::MS_SPLIT_MB,
        archive_level: 9,
    })
}
