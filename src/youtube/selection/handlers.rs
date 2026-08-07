use frankenstein::{client_reqwest::Bot, types::CallbackQuery};

use crate::database::postgresql::PostgresDatabase;
use crate::i18n::{entities_for_text, tf};
use crate::rank;

use super::super::download::{
    Selection, SelectionView, SubtitleMode, YoutubeRequest, get_request, spawn_download,
    with_selection,
};
use super::super::trace::log_trace;
use super::super::types::VideoCodec;
use super::buttons::{answer, quality_label};
use super::constants::*;
use super::panel::{extract_message, refresh_full_panel, refresh_keyboard};
use frankenstein::{AsyncTelegramApi, methods::EditMessageTextParams};

pub async fn handle_selection_callback(
    api: &Bot,
    callback_query: &CallbackQuery,
    database: &Option<PostgresDatabase>,
) -> bool {
    let Some(data) = callback_query.data.as_deref() else {
        return false;
    };
    if !data.starts_with(CB_SELECTION_PREFIX) {
        return false;
    }

    if data == CB_NOP {
        answer(api, callback_query, "").await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_CODEC) {
        handle_codec_toggle(api, callback_query, rest).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_AUDIO) {
        handle_audio_toggle(api, callback_query, rest).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_SUB_TOGGLE) {
        handle_sub_toggle(api, callback_query, rest, database).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_SUB_MENU) {
        handle_sub_view_change(api, callback_query, rest, Some(0)).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_SUB_BACK) {
        handle_sub_view_change(api, callback_query, rest, None).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_SUB_PAGE) {
        handle_sub_page(api, callback_query, rest).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_SUB_MODE) {
        handle_sub_mode_toggle(api, callback_query, rest, database).await;
        return true;
    }
    if let Some(rest) = data.strip_prefix(CB_GO) {
        handle_go(api, callback_query, rest, database).await;
        return true;
    }
    answer(api, callback_query, "").await;
    true
}

async fn parse_rid_and_req(
    api: &Bot,
    cq: &CallbackQuery,
    rest: &str,
) -> Option<(u64, YoutubeRequest)> {
    let request_id = rest.parse::<u64>().ok()?;
    let req = get_request(request_id);
    if req.is_none() {
        answer(api, cq, "youtube.download.request_expired").await;
    }
    req.map(|r| (request_id, r))
}

async fn handle_codec_toggle(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some((request_id, codec_key)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let (Ok(request_id), Some(codec)) =
        (request_id.parse::<u64>(), VideoCodec::from_key(codec_key))
    else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let trace_id = req.trace_id;
    let changed = with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut()
            && sel.codec != codec
        {
            sel.codec = codec;
            return true;
        }
        false
    });
    log_trace(
        trace_id,
        "selection_codec",
        &format!(
            "request_id={request_id} codec={} changed={changed}",
            codec.key()
        ),
    );
    if changed && let Some(msg) = extract_message(cq) {
        refresh_full_panel(api, msg, &req, request_id).await;
    }
    answer(api, cq, "").await;
}

async fn handle_audio_toggle(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some((request_id, idx_str)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let (Ok(request_id), Ok(idx)) = (request_id.parse::<u64>(), idx_str.parse::<usize>()) else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let Some(lang) = req.audio_languages.get(idx).map(|l| l.code.clone()) else {
        answer(api, cq, "").await;
        return;
    };
    let trace_id = req.trace_id;
    let changed = with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut()
            && sel.audio_lang.as_deref() != Some(&lang)
        {
            sel.audio_lang = Some(lang.clone());
            return true;
        }
        false
    });
    log_trace(
        trace_id,
        "selection_audio",
        &format!("request_id={request_id} lang={lang} changed={changed}"),
    );
    if changed && let Some(msg) = extract_message(cq) {
        refresh_keyboard(api, msg, &req, request_id).await;
    }
    answer(api, cq, "").await;
}

async fn handle_sub_toggle(
    api: &Bot,
    cq: &CallbackQuery,
    rest: &str,
    database: &Option<PostgresDatabase>,
) {
    let Some((request_id, idx_str)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let (Ok(request_id), Ok(idx)) = (request_id.parse::<u64>(), idx_str.parse::<usize>()) else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let Some(lang) = req.subtitle_languages.get(idx).map(|l| l.code.clone()) else {
        answer(api, cq, "").await;
        return;
    };
    let trace_id = req.trace_id;

    // rank check — زیرنویس فقط سپهبد به بالا
    if let (Some(uid), Some(db)) = (req.user_id, database.as_ref()) {
        let user_rank = rank::effective_rank(db.client(), uid).await;
        if !user_rank.can_subtitle_mux() {
            log_trace(
                trace_id,
                "sub_paywall",
                &format!("user_id={uid} rank={}", user_rank.as_str()),
            );
            answer(api, cq, "").await;
            if let Some(msg) = extract_message(cq) {
                crate::rank::paywall::block_feature(
                    api,
                    msg.chat.id,
                    &crate::i18n::t("youtube.subtitle_feature"),
                    rank::types::Rank::Sepahbod,
                )
                .await;
            }
            return;
        }
    }

    // Toggle subtitle selection: if already selected, remove it; if not, add it.
    // If subtitle_mode is Hardsub and adding a new lang, keep only the newly added lang.
    let (toggled, total) = with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut() {
            if let Some(pos) = sel.subtitle_langs.iter().position(|l| l == &lang) {
                sel.subtitle_langs.remove(pos);
                (true, sel.subtitle_langs.len())
            } else {
                if sel.subtitle_mode == SubtitleMode::Hardsub {
                    sel.subtitle_langs.clear();
                }
                sel.subtitle_langs.push(lang.clone());
                (true, sel.subtitle_langs.len())
            }
        } else {
            (false, 0)
        }
    });
    log_trace(
        trace_id,
        "selection_sub_toggle",
        &format!("request_id={request_id} lang={lang} toggled={toggled} total_selected={total}"),
    );
    if toggled && let Some(msg) = extract_message(cq) {
        refresh_keyboard(api, msg, &req, request_id).await;
    }
    answer(api, cq, "").await;
}

async fn handle_sub_mode_toggle(
    api: &Bot,
    cq: &CallbackQuery,
    rest: &str,
    database: &Option<PostgresDatabase>,
) {
    let Some((request_id, mode_str)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let Ok(request_id) = request_id.parse::<u64>() else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let new_mode = match mode_str {
        "file" => SubtitleMode::File,
        "embedded" => SubtitleMode::Embedded,
        "hardsub" => SubtitleMode::Hardsub,
        _ => {
            answer(api, cq, "").await;
            return;
        }
    };
    let trace_id = req.trace_id;

    // rank check — فایل جداگانه فقط سپهبد به بالا
    if new_mode == SubtitleMode::File
        && let (Some(uid), Some(db)) = (req.user_id, database.as_ref())
    {
        let user_rank = rank::effective_rank(db.client(), uid).await;
        if !user_rank.can_subtitle_file() {
            log_trace(
                trace_id,
                "sub_file_paywall",
                &format!("user_id={uid} rank={}", user_rank.as_str()),
            );
            answer(api, cq, "").await;
            if let Some(msg) = extract_message(cq) {
                crate::rank::paywall::block_feature(
                    api,
                    msg.chat.id,
                    &crate::i18n::t("youtube.subtitle_file_feature"),
                    rank::types::Rank::Sepahbod,
                )
                .await;
            }
            return;
        }
    }

    // rank check — هاردساب فقط اسفندیار به بالا
    if new_mode == SubtitleMode::Hardsub
        && let (Some(uid), Some(db)) = (req.user_id, database.as_ref())
    {
        let user_rank = rank::effective_rank(db.client(), uid).await;
        if !user_rank.can_subtitle_hardcode() {
            log_trace(
                trace_id,
                "sub_hardsub_paywall",
                &format!("user_id={uid} rank={}", user_rank.as_str()),
            );
            answer(api, cq, "").await;
            if let Some(msg) = extract_message(cq) {
                crate::rank::paywall::block_feature(
                    api,
                    msg.chat.id,
                    &crate::i18n::t("youtube.subtitle_hardsub_feature"),
                    rank::types::Rank::Esfandyar,
                )
                .await;
            }
            return;
        }
    }

    let changed = with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut() {
            let mode_changed = sel.subtitle_mode != new_mode;
            sel.subtitle_mode = new_mode;

            // If switching to Hardsub and multiple subtitle languages are selected, keep only the latest.
            if new_mode == SubtitleMode::Hardsub && sel.subtitle_langs.len() > 1 {
                if let Some(last) = sel.subtitle_langs.pop() {
                    sel.subtitle_langs = vec![last];
                }
                return true;
            }

            mode_changed
        } else {
            false
        }
    });
    log_trace(
        trace_id,
        "selection_sub_mode",
        &format!("request_id={request_id} mode={mode_str} changed={changed}"),
    );
    if changed && let Some(msg) = extract_message(cq) {
        refresh_keyboard(api, msg, &req, request_id).await;
    }
    answer(api, cq, "").await;
}

async fn handle_sub_view_change(api: &Bot, cq: &CallbackQuery, rest: &str, page: Option<usize>) {
    let Some((request_id, req)) = parse_rid_and_req(api, cq, rest).await else {
        return;
    };
    let trace_id = req.trace_id;
    with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut() {
            sel.view = match page {
                Some(p) => SelectionView::SubMenu(p),
                None => SelectionView::Main,
            };
        }
    });
    log_trace(
        trace_id,
        "selection_view",
        &format!(
            "request_id={request_id} view={}",
            match page {
                Some(p) => format!("submenu:{p}"),
                None => "main".to_string(),
            }
        ),
    );
    if let Some(msg) = extract_message(cq) {
        refresh_keyboard(api, msg, &req, request_id).await;
    }
    answer(api, cq, "").await;
}

async fn handle_sub_page(api: &Bot, cq: &CallbackQuery, rest: &str) {
    let Some((request_id, page_str)) = rest.split_once(':') else {
        answer(api, cq, "").await;
        return;
    };
    let (Ok(request_id), Ok(page)) = (request_id.parse::<u64>(), page_str.parse::<usize>()) else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let trace_id = req.trace_id;
    with_selection(&req, |slot| {
        if let Some(sel) = slot.as_mut() {
            sel.view = SelectionView::SubMenu(page);
        }
    });
    log_trace(
        trace_id,
        "selection_sub_page",
        &format!("request_id={request_id} page={page}"),
    );
    if let Some(msg) = extract_message(cq) {
        refresh_keyboard(api, msg, &req, request_id).await;
    }
    answer(api, cq, "").await;
}

async fn handle_go(api: &Bot, cq: &CallbackQuery, rest: &str, database: &Option<PostgresDatabase>) {
    let Ok(request_id) = rest.parse::<u64>() else {
        answer(api, cq, "").await;
        return;
    };
    let Some(req) = get_request(request_id) else {
        answer(api, cq, "youtube.download.request_expired").await;
        return;
    };
    let Some(message) = extract_message(cq) else {
        answer(api, cq, "").await;
        return;
    };
    let trace_id = req.trace_id;
    if let Some(uid) = req.user_id {
        log_actor_id!("yt", trace_id, uid, "clicked" => "yt:s:go");
    }
    let selection = with_selection(&req, |slot| slot.clone()).unwrap_or_else(|| {
        log_trace(
            trace_id,
            "selection_go_missing",
            "no selection present, falling back",
        );
        Selection {
            height: req.formats.first().map(|f| f.height).unwrap_or(720),
            codec: req
                .formats
                .first()
                .map(|f| f.codec)
                .unwrap_or(VideoCodec::H264),
            audio_lang: None,
            subtitle_langs: Vec::new(),
            subtitle_mode: SubtitleMode::Embedded,
            view: SelectionView::Main,
            audio_only: None,
        }
    });
    log_trace(
        trace_id,
        "selection_confirm",
        &format!(
            "request_id={request_id} height={} codec={} audio={:?} subs={:?}",
            selection.height,
            selection.codec.key(),
            selection.audio_lang,
            selection.subtitle_langs
        ),
    );

    // ── چک ترافیک (روزانه + ماهانه) قبل از شروع دانلود ──
    if let (Some(uid), Some(db)) = (req.user_id, database.as_ref()) {
        let estimated = estimate_bytes(&req, &selection);
        let user_rank = rank::effective_rank(db.client(), uid).await;
        let daily_limit = user_rank.daily_traffic_bytes();
        let monthly_limit = user_rank.monthly_traffic_bytes();
        let first_upload_at = rank::quota::get_first_upload_at(db.client(), uid)
            .await
            .unwrap_or_else(now_epoch);
        let daily_used = rank::quota::get_daily_traffic(db.client(), uid)
            .await
            .unwrap_or(0) as u64;
        let monthly_used = rank::quota::get_monthly_traffic(db.client(), uid, first_upload_at)
            .await
            .unwrap_or(0) as u64;
        let daily_remaining = daily_limit.saturating_sub(daily_used);
        let monthly_remaining = monthly_limit.saturating_sub(monthly_used);

        let block = if daily_remaining == 0 {
            let label = tf(
                "youtube.traffic_daily_limit",
                &[("limit", &fmt_traffic_fa(daily_limit))],
            );
            Some((label, user_rank.traffic_daily_next_rank()))
        } else if monthly_remaining == 0 {
            let label = tf(
                "youtube.traffic_monthly_limit",
                &[("limit", &fmt_traffic_fa(monthly_limit))],
            );
            Some((label, user_rank.traffic_monthly_next_rank()))
        } else if estimated > 0 && (estimated > daily_remaining || estimated > monthly_remaining) {
            let remaining = daily_remaining.min(monthly_remaining);
            let next = if daily_remaining <= monthly_remaining {
                user_rank.traffic_daily_next_rank()
            } else {
                user_rank.traffic_monthly_next_rank()
            };
            let label = tf(
                "youtube.traffic_file_too_big",
                &[
                    ("size", &fmt_traffic_fa(estimated)),
                    ("remaining", &fmt_traffic_fa(remaining)),
                ],
            );
            Some((label, next))
        } else {
            None
        };

        if let Some((label, next_rank)) = block {
            log_trace(
                trace_id,
                "traffic_paywall",
                &format!(
                    "user_id={uid} rank={} est={estimated} daily_rem={daily_remaining} monthly_rem={monthly_remaining}",
                    user_rank.as_str()
                ),
            );
            answer(api, cq, "").await;
            if let Some(min_rank) = next_rank {
                crate::rank::paywall::block_limit(api, message.chat.id, &label, min_rank).await;
            } else {
                let _ = crate::bot::send_text(api, message.chat.id, &label).await;
            }
            return;
        }
    }

    answer(api, cq, "").await;
    let quality_lbl = quality_label(selection.height);
    let text = tf("youtube.download.starting", &[("quality", &quality_lbl)]);
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(message.chat.id)
        .message_id(message.message_id)
        .text(text)
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    if let Err(e) = api.edit_message_text(&params).await {
        log_trace(trace_id, "selection_start_edit_failed", &e.to_string());
    }
    crate::stats::record_event_user(
        req.user_id.unwrap_or(0),
        "youtube",
        &format!("q{}_{}", selection.height, selection.codec.key()),
        "go",
        0,
    )
    .await;
    spawn_download(
        api.clone(),
        request_id,
        selection,
        message.chat.id,
        message.message_id,
    );
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// تخمین حجم فایل ویدیو از bitrate × duration (همسان با نمایش پنل — فقط ویدیو).
/// اگه bitrate یا duration نبود → 0 (یعنی نامشخص، چک «فایل بزرگ» اعمال نمی‌شه).
fn estimate_bytes(req: &YoutubeRequest, sel: &Selection) -> u64 {
    let Some(dur) = req.duration.filter(|&d| d > 0) else {
        return 0;
    };
    let Some(fmt) = req
        .formats
        .iter()
        .find(|f| f.height == sel.height && f.codec == sel.codec)
    else {
        return 0;
    };
    let Some(kbps) = fmt.bitrate.filter(|&b| b > 0.0) else {
        return 0;
    };
    (kbps * 1000.0 / 8.0 * dur as f64) as u64
}

/// قالب‌بندی حجم به فارسی: «۵ گیگابایت» / «۷۵۰ مگابایت».
fn fmt_traffic_fa(bytes: u64) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    const MB: f64 = (1u64 << 20) as f64;
    let b = bytes as f64;
    let (num, unit) = if b >= GB {
        let g = b / GB;
        // عدد صحیح اگه گرد، وگرنه یک رقم اعشار
        if (g.round() - g).abs() < 0.05 {
            (
                format!("{:.0}", g.round()),
                crate::i18n::t("youtube.unit_gb"),
            )
        } else {
            (format!("{g:.1}"), crate::i18n::t("youtube.unit_gb"))
        }
    } else {
        (
            format!("{:.0}", (b / MB).round()),
            crate::i18n::t("youtube.unit_mb"),
        )
    };
    format!("{} {}", crate::i18n::to_fa_digits(&num), unit)
}
