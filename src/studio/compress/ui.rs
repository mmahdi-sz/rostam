use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::{InlineKeyboardMarkup, ReplyMarkup},
};

use super::calc::calculate_target_bitrate_kbps;
use super::session::CompressSession;
use crate::bot::constants::{
    CB_START_STUDIO, CB_STUDIO_COMPRESS_CANCEL, CB_STUDIO_COMPRESS_START,
};
use crate::emoji::panel::{btn_icon, btn_icon_danger, btn_icon_primary, btn_icon_success};
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::log::next_trace_id;

/// Renders the inline keyboard for the compression menu.
pub fn build_compress_keyboard(session: &CompressSession) -> InlineKeyboardMarkup {
    let mut rows = Vec::new();

    // Section 1: Codec
    let codecs = [
        ("h264", "H.264"),
        ("h265", "H.265"),
        ("vp9", "VP9"),
        ("av1", "AV1"),
    ];
    let mut codec_row = Vec::new();
    for (key, label) in codecs {
        let cb = format!("stc:set:c:{key}");
        let btn = if session.codec == key {
            btn_icon_success(label, &cb, "")
        } else {
            btn_icon(label, &cb, "")
        };
        codec_row.push(btn);
    }
    rows.push(codec_row);

    // Section 2: Resolution (Filtered by <= orig_h)
    let res_matrix: &[&[(u32, &str)]] = &[
        &[(2160, "2160p (4K)"), (1440, "1440p (2K)")],
        &[(1080, "1080p (fullHD)"), (720, "720p (HD)")],
        &[
            (480, "480p (SD)"),
            (360, "360p"),
            (240, "240p"),
            (144, "144p"),
        ],
    ];

    for row in res_matrix {
        let mut res_row = Vec::new();
        for &(h, label) in *row {
            if h <= session.orig_h {
                let cb = format!("stc:set:r:{h}");
                let btn = if session.res_h == h {
                    btn_icon_success(label, &cb, "")
                } else {
                    btn_icon(label, &cb, "")
                };
                res_row.push(btn);
            }
        }
        if !res_row.is_empty() {
            rows.push(res_row);
        }
    }

    // Section 3: FPS (Filtered by <= orig_fps)
    let fps_matrix: &[&[u32]] = &[&[60, 45, 30, 24], &[20, 15, 13]];

    for row in fps_matrix {
        let mut fps_row = Vec::new();
        for &f in *row {
            if f <= session.orig_fps {
                let label = format!("{f} fps");
                let cb = format!("stc:set:f:{f}");
                let btn = if session.fps == f {
                    btn_icon_success(&label, &cb, "")
                } else {
                    btn_icon(&label, &cb, "")
                };
                fps_row.push(btn);
            }
        }
        if !fps_row.is_empty() {
            rows.push(fps_row);
        }
    }

    // Section 4: Bitrate Ratio (Calculated kbps)
    let br_matrix: &[&[u32]] = &[&[100, 75, 50], &[25, 16, 12]];

    for row in br_matrix {
        let mut br_row = Vec::new();
        for &r in *row {
            let kbps = calculate_target_bitrate_kbps(session, session.res_h, r);
            let label = format!("{kbps} kbps");
            let cb = format!("stc:set:b:{r}");
            let btn = if session.br_ratio == r {
                btn_icon_success(&label, &cb, "")
            } else {
                btn_icon(&label, &cb, "")
            };
            br_row.push(btn);
        }
        if !br_row.is_empty() {
            rows.push(br_row);
        }
    }

    // Section 5: Actions
    rows.push(vec![btn_icon_success(
        &t("studio.compress.confirm_btn"),
        CB_STUDIO_COMPRESS_START,
        "rocket",
    )]);
    rows.push(vec![btn_icon_primary(
        &t("studio.back_to_studio"),
        CB_START_STUDIO,
        "back",
    )]);

    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

/// Renders the MarkdownV2 text for the compression menu.
pub fn build_compress_text(session: &CompressSession) -> String {
    let orig_res = format!("{}x{}", session.orig_w, session.orig_h);
    let orig_bitrate_kbps = session.orig_bitrate / 1000;
    let orig_size_mb = (session.orig_size_bytes as f64) / (1024.0 * 1024.0);
    let orig_size_str = format!("{orig_size_mb:.1}");

    let sel_codec = match session.codec.as_str() {
        "h264" => "H.264",
        "h265" => "H.265 (HEVC)",
        "vp9" => "VP9",
        "av1" => "AV1",
        _ => session.codec.as_str(),
    };
    let sel_res = format!("{}p", session.res_h);

    let sel_br_kbps = calculate_target_bitrate_kbps(session, session.res_h, session.br_ratio);
    let sel_br_label = format!("{sel_br_kbps} kbps");

    let container = if session.codec == "h264" {
        ".mp4"
    } else {
        ".mkv"
    };

    let raw = tf(
        "studio.compress.menu_title",
        &[
            ("orig_res", &md_escape(&orig_res)),
            ("orig_fps", &session.orig_fps.to_string()),
            ("orig_codec", &md_escape(&session.orig_codec)),
            ("orig_bitrate", &orig_bitrate_kbps.to_string()),
            ("orig_size", &md_escape(&orig_size_str)),
            ("sel_codec", &md_escape(sel_codec)),
            ("sel_res", &md_escape(&sel_res)),
            ("sel_fps", &session.fps.to_string()),
            ("sel_br_label", &md_escape(&sel_br_label)),
            ("container", &md_escape(container)),
        ],
    );

    apply_premium_to_md(&raw)
}

pub async fn send_compress_prompt_new_msg(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.set(user_id, FlowState::AwaitingStudioCompressVideo);
    log_actor_id!("studio_compress", trace_id, user_id, "rearm" => "prompt");

    let text = apply_premium_to_md(&t("studio.compress.send_video_prompt"));
    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("studio.compress.cancel_btn"),
            CB_STUDIO_COMPRESS_CANCEL,
            "cancel",
        )]])
        .build();

    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(kb))
        .build();

    let _ = api.send_message(&params).await;
}
