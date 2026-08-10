use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{DeleteMessageParams, EditMessageTextParams, SendDocumentParams, SendMessageParams},
    types::{
        ButtonStyle, InlineKeyboardMarkup, KeyboardButton, Message, ReplyKeyboardMarkup,
        ReplyMarkup, ReplyParameters,
    },
};

use super::config::{CompressAlgo, CompressConfig, CompressFmt};
use super::engine::{CompressError, run_compress};
use crate::bot::{download_telegram_file, edit_to_tools, send_text_with_back};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::flow::CompressFileEntry;
use crate::emoji::panel::{
    btn_icon, btn_icon_danger, btn_icon_plain, btn_icon_primary, btn_icon_success,
};
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t, tf};
use crate::log::next_trace_id;
use crate::rank::{self, quota::QuotaKind};

const SEP_BASE: &str = "http://127.0.0.1:6589";

/// Cancel flag per user so the "Cancel" button on progress message works.
/// Kills 7z/rar process to free CPU instead of discarding output.
static ACTIVE_FC_JOBS: LazyLock<Mutex<HashMap<i64, Arc<AtomicBool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn remove_active_fc_job(user_id: i64) {
    if let Ok(mut jobs) = ACTIVE_FC_JOBS.lock() {
        jobs.remove(&user_id);
    }
}

pub const CB_TOOLS_FILECOMPRESS: &str = "tools:fc";
pub const CB_FC_PREFIX: &str = "fc:";
pub const CB_FC_CANCEL: &str = "fc:cancel";

// ── Keyboards ──────────────────────────────────────────────────────────────────

fn options_keyboard(config: &CompressConfig) -> InlineKeyboardMarkup {
    let mut rows = Vec::new();

    // Row 1: Format selection (ZIP / 7Z / RAR)
    let zip_btn = if config.fmt == CompressFmt::Zip {
        btn_icon_success("ZIP", "fc:fmt:zip", "pack_folder")
    } else {
        btn_icon_plain("ZIP", "fc:fmt:zip", "pack_folder")
    };
    let sz_btn = if config.fmt == CompressFmt::SevenZ {
        btn_icon_success("7Z", "fc:fmt:7z", "7zip_logo")
    } else {
        btn_icon_plain("7Z", "fc:fmt:7z", "7zip_logo")
    };
    let rar_btn = if config.fmt == CompressFmt::Rar {
        btn_icon_success("RAR", "fc:fmt:rar", "rar_logo")
    } else {
        btn_icon_plain("RAR", "fc:fmt:rar", "rar_logo")
    };
    // zstd icon not set yet — icon name will be filled later
    let zstd_btn = if config.fmt == CompressFmt::Zstd {
        btn_icon_success("ZSTD", "fc:fmt:zstd", "")
    } else {
        btn_icon_plain("ZSTD", "fc:fmt:zstd", "")
    };
    rows.push(vec![zip_btn, sz_btn, rar_btn, zstd_btn]);

    // Row 2: Algorithm selection (7Z only)
    if config.fmt == CompressFmt::SevenZ {
        let lzma2_btn = if config.algo == CompressAlgo::Lzma2 {
            btn_icon_success("LZMA2", "fc:algo:lzma2", "sparkles")
        } else {
            btn_icon_plain("LZMA2", "fc:algo:lzma2", "")
        };
        let ppmd_btn = if config.algo == CompressAlgo::Ppmd {
            btn_icon_success("PPMd", "fc:algo:ppmd", "sparkles")
        } else {
            btn_icon_plain("PPMd", "fc:algo:ppmd", "")
        };
        let bzip2_btn = if config.algo == CompressAlgo::Bzip2 {
            btn_icon_success("BZip2", "fc:algo:bzip2", "sparkles")
        } else {
            btn_icon_plain("BZip2", "fc:algo:bzip2", "")
        };
        rows.push(vec![lzma2_btn, ppmd_btn, bzip2_btn]);
    }

    // Row 3: Compression Level Title (BLUE per user request!)
    let level_text = if config.level == 0 {
        t("fc.level_label_store")
    } else {
        t("fc.level_label").replace("{level}", &config.level.to_string())
    };
    rows.push(vec![btn_icon(&level_text, "fc:noop", "panel")]);

    // Row 4: Compression Level Controls (- and +)
    rows.push(vec![
        btn_icon("\u{200B}", "fc:lvl:down", "prev"),
        btn_icon("\u{200B}", "fc:lvl:up", "next"),
    ]);

    // Row 5: Password Encryption Toggle — Formats without password support do not get button.
    if config.fmt.supports_password() {
        let (pass_label, pass_btn) = if config.password.is_some() {
            (
                t("fc.toggle_password"),
                btn_icon_success(&t("fc.status_on"), "fc:toggle:pass", "check"),
            )
        } else {
            (
                t("fc.toggle_password"),
                btn_icon_danger(&t("fc.status_off"), "fc:toggle:pass", "cross"),
            )
        };
        rows.push(vec![
            btn_icon_plain(&pass_label, "fc:toggle:pass", "warning"),
            pass_btn,
        ]);
    }

    // Row 6: Split into parts Toggle
    if config.fmt.supports_split() {
        let (split_label, split_btn) = if let Some(mb) = config.split_mb {
            (
                t("fc.toggle_split"),
                btn_icon_success(&format!("{mb} MB"), "fc:toggle:split", "check"),
            )
        } else {
            (
                t("fc.toggle_split"),
                btn_icon_danger(&t("fc.status_off"), "fc:toggle:split", "cross"),
            )
        };
        rows.push(vec![
            btn_icon_plain(&split_label, "fc:toggle:split", "replace_mode"),
            split_btn,
        ]);
    }

    // Split size controls if split enabled
    if let Some(mb) = config.split_mb {
        rows.push(vec![btn_icon_plain(
            &t("fc.part_size_label").replace("{mb}", &mb.to_string()),
            "fc:noop",
            "info",
        )]);
        rows.push(vec![
            btn_icon_plain("+5", "fc:part:+5", ""),
            btn_icon_plain("+10", "fc:part:+10", ""),
            btn_icon_plain("+25", "fc:part:+25", ""),
            btn_icon_plain("+50", "fc:part:+50", ""),
            btn_icon_plain("+100", "fc:part:+100", ""),
            btn_icon_plain("+250", "fc:part:+250", ""),
        ]);
        rows.push(vec![
            btn_icon_plain("-5", "fc:part:-5", ""),
            btn_icon_plain("-10", "fc:part:-10", ""),
            btn_icon_plain("-25", "fc:part:-25", ""),
            btn_icon_plain("-50", "fc:part:-50", ""),
            btn_icon_plain("-100", "fc:part:-100", ""),
            btn_icon_plain("-250", "fc:part:-250", ""),
        ]);
    }

    // Row 8 (7Z only): Header Encryption (Obfuscate) Toggle
    if config.fmt == CompressFmt::SevenZ {
        let (obf_label, obf_btn) = if config.obfuscate {
            (
                t("fc.toggle_obfuscate"),
                btn_icon_success(&t("fc.status_on"), "fc:toggle:obfuscate", "check"),
            )
        } else {
            (
                t("fc.toggle_obfuscate"),
                btn_icon_danger(&t("fc.status_off"), "fc:toggle:obfuscate", "cross"),
            )
        };
        rows.push(vec![
            btn_icon_plain(&obf_label, "fc:toggle:obfuscate", "eye"),
            obf_btn,
        ]);
    }

    // Row 9: Solid Mode Toggle
    // tar.zst is always a single stream, so solid mode is not selectable.
    if config.fmt.supports_solid() {
        let solid_btn = if config.solid {
            btn_icon_primary(&t("fc.solid_mode_solid"), "fc:toggle:solid", "pack_folder")
        } else {
            btn_icon_success(&t("fc.solid_mode_normal"), "fc:toggle:solid", "rocket")
        };
        rows.push(vec![
            btn_icon(&t("fc.toggle_solid"), "fc:toggle:solid", "pack_folder"),
            solid_btn,
        ]);
    }

    // Row 10: Confirm + Cancel
    rows.push(vec![
        btn_icon_success(&t("fc.confirm_button"), "fc:confirm", "confirm"),
        btn_icon_plain(&t("start.back"), CB_FC_CANCEL, "back"),
    ]);

    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

/// Cancel button only — for password prompt step.
fn cancel_only_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("fc.cancel_button"),
            CB_FC_CANCEL,
            "cancel",
        )]])
        .build()
}

/// Cancel button on progress message — cancels active job.
fn job_cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("fc.cancel_button"),
            "fc:jobcancel",
            "cancel",
        )]])
        .build()
}

fn done_reply_keyboard() -> ReplyKeyboardMarkup {
    let confirm_icon = t("emoji.panel.icons.confirm");
    let cancel_icon = t("emoji.panel.icons.cancel");
    ReplyKeyboardMarkup::builder()
        .keyboard(vec![vec![
            KeyboardButton::builder()
                .text(t("fc.done_upload_button"))
                .style(ButtonStyle::Success)
                .icon_custom_emoji_id(confirm_icon)
                .build(),
            KeyboardButton::builder()
                .text(t("fc.cancel_button"))
                .style(ButtonStyle::Danger)
                .icon_custom_emoji_id(cancel_icon)
                .build(),
        ]])
        .resize_keyboard(true)
        .one_time_keyboard(true)
        .build()
}

// ── Entry point ────────────────────────────────────────────────────────────────

pub async fn enter_filecompress(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    crate::log_actor_id!("filecompress", trace_id, user_id, "clicked" => CB_TOOLS_FILECOMPRESS);
    let config = CompressConfig::default();
    flow_manager.set(
        user_id,
        FlowState::AwaitingCompressOptions {
            config: config.clone(),
        },
    );

    show_options_menu(api, chat_id, message_id, &config).await;
}

// ── Callback Handler ───────────────────────────────────────────────────────────

/// Callback response; populated text = transient toast on user screen.
async fn fc_answer(api: &Bot, cb_id: &str, text: Option<String>) {
    let b = frankenstein::methods::AnswerCallbackQueryParams::builder().callback_query_id(cb_id);
    let _ = match text {
        Some(txt) => api.answer_callback_query(&b.text(txt).build()).await,
        None => api.answer_callback_query(&b.build()).await,
    };
}

pub async fn handle_fc_callback(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
    action: &str,
    cb_id: &str,
    _database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    crate::log_ev!("filecompress", trace_id, "callback", "action" => action, "user_id" => user_id);

    // Instant ack for all callbacks except those with custom toast/alert
    if !matches!(action, "lvl:up" | "lvl:down" | "toggle:obfuscate") {
        fc_answer(api, cb_id, None).await;
    }

    if action == "cancel" {
        flow_manager.clear(user_id);
        let _ = edit_to_tools(api, chat_id, message_id).await;
        return;
    }

    if action == "jobcancel" {
        crate::log_ev!("filecompress", trace_id, "job_cancelled", "user_id" => user_id);
        if let Ok(mut jobs) = ACTIVE_FC_JOBS.lock() {
            if let Some(flag) = jobs.remove(&user_id) {
                flag.store(true, Ordering::Relaxed);
            }
        }
        flow_manager.clear(user_id);
        let params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(&apply_premium_to_md(&t("fc.cancelled")))
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        let _ = api.edit_message_text(&params).await;
        return;
    }

    if action == "noop" {
        return;
    }

    let state = flow_manager.get(user_id);

    match action {
        "fmt:zip" | "fmt:7z" | "fmt:rar" | "fmt:zstd" => {
            let fmt_str = &action["fmt:".len()..];
            let fmt = CompressFmt::from_str(fmt_str).unwrap_or(CompressFmt::SevenZ);
            let mut config = match state {
                FlowState::AwaitingCompressOptions { config } => config,
                _ => CompressConfig::default(),
            };
            config.fmt = fmt;
            // Settings unsupported by new format are cleared, not just hidden
            if config.level > config.fmt.max_level() {
                config.level = config.fmt.max_level();
            }
            if !config.fmt.supports_password() {
                config.password = None;
                config.obfuscate = false;
            }
            if !config.fmt.supports_split() {
                config.split_mb = None;
            }
            flow_manager.set(
                user_id,
                FlowState::AwaitingCompressOptions {
                    config: config.clone(),
                },
            );
            show_options_menu(api, chat_id, message_id, &config).await;
        }
        "algo:lzma2" | "algo:ppmd" | "algo:bzip2" => {
            let algo_str = &action["algo:".len()..];
            let algo = CompressAlgo::from_str(algo_str).unwrap_or(CompressAlgo::Lzma2);
            let mut config = match state {
                FlowState::AwaitingCompressOptions { config } => config,
                _ => CompressConfig::default(),
            };
            config.algo = algo;
            flow_manager.set(
                user_id,
                FlowState::AwaitingCompressOptions {
                    config: config.clone(),
                },
            );
            show_options_menu(api, chat_id, message_id, &config).await;
        }
        "lvl:up" | "lvl:down" => {
            let mut config = match state {
                FlowState::AwaitingCompressOptions { config } => config,
                _ => return,
            };
            let max_level = config.fmt.max_level();
            if action == "lvl:up" && config.level < max_level {
                config.level += 1;
            } else if action == "lvl:down" && config.level > 0 {
                config.level -= 1;
            }
            // On max level, show transient toast with format name and ceiling
            let toast = if config.level >= max_level {
                Some(tf(
                    "fc.max_level_notice",
                    &[
                        ("fmt", config.fmt.as_str()),
                        ("max", &max_level.to_string()),
                    ],
                ))
            } else {
                None
            };
            fc_answer(api, cb_id, toast).await;
            flow_manager.set(
                user_id,
                FlowState::AwaitingCompressOptions {
                    config: config.clone(),
                },
            );
            show_options_menu(api, chat_id, message_id, &config).await;
        }
        "toggle:pass" => {
            let mut config = match state {
                FlowState::AwaitingCompressOptions { config } => config,
                _ => return,
            };
            // Button hidden for this format, but old inline message is still clickable
            if !config.fmt.supports_password() {
                return;
            }
            if config.password.is_some() {
                config.password = None;
                config.obfuscate = false; // Turn off obfuscation if password removed
            } else {
                config.password = Some("".to_string()); // Flag password mode as active
            }
            flow_manager.set(
                user_id,
                FlowState::AwaitingCompressOptions {
                    config: config.clone(),
                },
            );
            show_options_menu(api, chat_id, message_id, &config).await;
        }
        "toggle:split" => {
            let mut config = match state {
                FlowState::AwaitingCompressOptions { config } => config,
                _ => return,
            };
            if !config.fmt.supports_split() {
                return;
            }
            if config.split_mb.is_some() {
                config.split_mb = None;
            } else {
                config.split_mb = Some(1000); // Default 1000MB
            }
            flow_manager.set(
                user_id,
                FlowState::AwaitingCompressOptions {
                    config: config.clone(),
                },
            );
            show_options_menu(api, chat_id, message_id, &config).await;
        }
        "toggle:obfuscate" => {
            let mut config = match state {
                FlowState::AwaitingCompressOptions { config } => config,
                _ => return,
            };
            if config.password.is_none() {
                // Cannot enable header obfuscation without password
                let params = frankenstein::methods::AnswerCallbackQueryParams::builder()
                    .callback_query_id(cb_id)
                    .text(t("fc.error.obfuscate_needs_password"))
                    .show_alert(true)
                    .build();
                let _ = api.answer_callback_query(&params).await;
                return;
            }
            fc_answer(api, cb_id, None).await;
            config.obfuscate = !config.obfuscate;
            flow_manager.set(
                user_id,
                FlowState::AwaitingCompressOptions {
                    config: config.clone(),
                },
            );
            show_options_menu(api, chat_id, message_id, &config).await;
        }
        "toggle:solid" => {
            let mut config = match state {
                FlowState::AwaitingCompressOptions { config } => config,
                _ => return,
            };
            if !config.fmt.supports_solid() {
                return;
            }
            config.solid = !config.solid;
            flow_manager.set(
                user_id,
                FlowState::AwaitingCompressOptions {
                    config: config.clone(),
                },
            );
            show_options_menu(api, chat_id, message_id, &config).await;
        }
        _ if action.starts_with("part:") => {
            let mut config = match state {
                FlowState::AwaitingCompressOptions { config } => config,
                _ => return,
            };
            if !config.fmt.supports_split() {
                return;
            }
            let current_mb = config.split_mb.unwrap_or(1000) as i32;
            let delta: i32 = action["part:".len()..].parse().unwrap_or(0);
            let new_mb = (current_mb + delta).clamp(5, 2000) as u32;
            config.split_mb = Some(new_mb);
            flow_manager.set(
                user_id,
                FlowState::AwaitingCompressOptions {
                    config: config.clone(),
                },
            );
            show_options_menu(api, chat_id, message_id, &config).await;
        }
        "confirm" => {
            let config = match state {
                FlowState::AwaitingCompressOptions { config } => config,
                _ => return,
            };

            if config.password.is_some() {
                flow_manager.set(user_id, FlowState::AwaitingCompressPassword { config });
                let params = EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(message_id)
                    .text(&apply_premium_to_md(&t("fc.ask_password")))
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(cancel_only_keyboard())
                    .build();
                let _ = api.edit_message_text(&params).await;
            } else {
                // Delete inline options message
                let _ = api
                    .delete_message(
                        &DeleteMessageParams::builder()
                            .chat_id(chat_id)
                            .message_id(message_id)
                            .build(),
                    )
                    .await;

                // Send ONE new message with instructions and reply keyboard
                let upload_text =
                    apply_premium_to_md(&t("fc.upload_prompt").replace("{count}", "0"));
                let send_res = api
                    .send_message(
                        &SendMessageParams::builder()
                            .chat_id(chat_id)
                            .text(&upload_text)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(ReplyMarkup::ReplyKeyboardMarkup(done_reply_keyboard()))
                            .build(),
                    )
                    .await;

                let prompt_msg_id = send_res.map(|m| m.result.message_id).unwrap_or(message_id);

                flow_manager.set(
                    user_id,
                    FlowState::AwaitingCompressFiles {
                        config: Box::new(config),
                        files: Vec::new(),
                        prompt_msg_id,
                    },
                );
            }
        }
        _ => {}
    }
}

async fn show_options_menu(api: &Bot, chat_id: i64, message_id: i32, config: &CompressConfig) {
    let key = match config.fmt {
        CompressFmt::SevenZ => "fc.welcome_7z",
        CompressFmt::Zstd => "fc.welcome_zstd",
        _ => "fc.welcome",
    };
    let text = apply_premium_to_md(&t(key));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(options_keyboard(config))
        .build();
    let r = api.edit_message_text(&params).await;
    if let Err(ref e) = r {
        crate::log_ev!("filecompress", 0, "show_options_menu_err", "err" => format!("{e:?}"));
    }
}

pub async fn send_options_menu(api: &Bot, chat_id: i64, config: &CompressConfig) {
    let key = match config.fmt {
        CompressFmt::SevenZ => "fc.welcome_7z",
        CompressFmt::Zstd => "fc.welcome_zstd",
        _ => "fc.welcome",
    };
    let text = apply_premium_to_md(&t(key));
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(options_keyboard(config)))
        .build();
    let _ = api.send_message(&params).await;
}

// ── Text Handler for Password ─────────────────────────────────────────────────

pub async fn handle_fc_password_text(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &FlowManager,
    mut config: CompressConfig,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    let password = message.text.as_deref().unwrap_or("").trim().to_string();

    // Delete password message immediately
    let del_p = DeleteMessageParams::builder()
        .chat_id(chat_id)
        .message_id(message.message_id)
        .build();
    let _ = api.delete_message(&del_p).await;

    config.password = Some(password);

    crate::log_ev!("filecompress", trace_id, "password_received", "user_id" => user_id);

    let upload_text = apply_premium_to_md(&t("fc.upload_prompt").replace("{count}", "0"));
    let send_res = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&upload_text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::ReplyKeyboardMarkup(done_reply_keyboard()))
                .build(),
        )
        .await;

    let prompt_msg_id = send_res
        .map(|m| m.result.message_id)
        .unwrap_or(message.message_id);

    flow_manager.set(
        user_id,
        FlowState::AwaitingCompressFiles {
            config: Box::new(config),
            files: Vec::new(),
            prompt_msg_id,
        },
    );
}

/// User sent file during password step: notify text is required, with cancel button.
pub async fn send_password_need_text(api: &Bot, chat_id: i64) {
    let _ = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&apply_premium_to_md(&t("fc.password_need_text")))
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(cancel_only_keyboard()))
                .build(),
        )
        .await;
}

fn extract_media_file_info(message: &Message, index: usize) -> Option<(String, String, u64)> {
    if let Some(doc) = &message.document {
        let name = doc
            .file_name
            .clone()
            .unwrap_or_else(|| format!("file_{}.bin", index + 1));
        return Some((doc.file_id.clone(), name, doc.file_size.unwrap_or(0) as u64));
    }
    if let Some(vid) = &message.video {
        let name = vid
            .file_name
            .clone()
            .unwrap_or_else(|| format!("video_{}.mp4", index + 1));
        return Some((vid.file_id.clone(), name, vid.file_size.unwrap_or(0) as u64));
    }
    if let Some(aud) = &message.audio {
        let name = aud
            .file_name
            .clone()
            .unwrap_or_else(|| format!("audio_{}.mp3", index + 1));
        return Some((aud.file_id.clone(), name, aud.file_size.unwrap_or(0) as u64));
    }
    if let Some(photos) = &message.photo {
        if let Some(p) = photos.last() {
            let name = format!("photo_{}.jpg", index + 1);
            return Some((p.file_id.clone(), name, p.file_size.unwrap_or(0) as u64));
        }
    }
    if let Some(v) = &message.voice {
        let name = format!("voice_{}.ogg", index + 1);
        return Some((v.file_id.clone(), name, v.file_size.unwrap_or(0) as u64));
    }
    if let Some(vn) = &message.video_note {
        let name = format!("video_note_{}.mp4", index + 1);
        return Some((vn.file_id.clone(), name, vn.file_size.unwrap_or(0) as u64));
    }
    if let Some(anim) = &message.animation {
        let name = anim
            .file_name
            .clone()
            .unwrap_or_else(|| format!("animation_{}.mp4", index + 1));
        return Some((
            anim.file_id.clone(),
            name,
            anim.file_size.unwrap_or(0) as u64,
        ));
    }
    None
}

// ── Document Intake Handler ────────────────────────────────────────────────────

pub async fn handle_fc_file(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;

    let (config, mut files, prompt_msg_id) = match flow_manager.get(user_id) {
        FlowState::AwaitingCompressFiles {
            config,
            files,
            prompt_msg_id,
        } => (*config, files, prompt_msg_id),
        FlowState::AwaitingCompressOptions { config } => {
            let upload_text = apply_premium_to_md(&t("fc.upload_prompt").replace("{count}", "0"));
            let send_res = api
                .send_message(
                    &SendMessageParams::builder()
                        .chat_id(chat_id)
                        .text(&upload_text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(ReplyMarkup::ReplyKeyboardMarkup(done_reply_keyboard()))
                        .build(),
                )
                .await;
            let prompt_id = send_res
                .map(|m| m.result.message_id)
                .unwrap_or(message.message_id);
            (config, Vec::new(), prompt_id)
        }
        _ => return,
    };

    let (file_id, filename, size) = match extract_media_file_info(message, files.len()) {
        Some(info) => info,
        None => return,
    };

    files.push(CompressFileEntry {
        file_id,
        filename,
        size,
    });

    crate::log_ev!(
        "filecompress",
        trace_id,
        "file_received",
        "count" => files.len(),
        "user_id" => user_id
    );

    let safe_filename = crate::youtube::escape_markdown_v2(&files.last().unwrap().filename);
    let count_str = files.len().to_string();
    let reply_text = apply_premium_to_md(
        &t("fc.file_received")
            .replace("{filename}", &safe_filename)
            .replace("{count}", &count_str),
    );
    let reply_params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&reply_text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_parameters(
            ReplyParameters::builder()
                .message_id(message.message_id)
                .build(),
        )
        .build();
    // Send ack asynchronously to prevent blocking Telegram update loop
    let api_ack = api.clone();
    crate::app::spawn_user_task(async move {
        let _ = api_ack.send_message(&reply_params).await;
    });

    if files.len() >= 20 {
        // Auto-start on reaching max 20 files
        flow_manager.clear(user_id);
        start_compression_task(
            api,
            chat_id,
            prompt_msg_id,
            user_id,
            config,
            files,
            trace_id,
            database,
            flow_manager,
        )
        .await;
    } else {
        flow_manager.set(
            user_id,
            FlowState::AwaitingCompressFiles {
                config: Box::new(config),
                files: files.clone(),
                prompt_msg_id,
            },
        );

        let text = apply_premium_to_md(&t("fc.upload_prompt").replace("{count}", &count_str));
        let params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(prompt_msg_id)
            .text(&text)
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        let api_edit = api.clone();
        crate::app::spawn_user_task(async move {
            let _ = api_edit.edit_message_text(&params).await;
        });
    }
}

// ── Done Text Handler ──────────────────────────────────────────────────────────

pub async fn handle_fc_done_text(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;

    let (config, files, prompt_msg_id) = match flow_manager.get(user_id) {
        FlowState::AwaitingCompressFiles {
            config,
            files,
            prompt_msg_id,
        } => (*config, files, prompt_msg_id),
        _ => return,
    };

    flow_manager.clear(user_id);

    if files.is_empty() {
        let _ = send_text_with_back(api, chat_id, &t("fc.error.no_files")).await;
        return;
    }

    start_compression_task(
        api,
        chat_id,
        prompt_msg_id,
        user_id,
        config,
        files,
        trace_id,
        database,
        flow_manager,
    )
    .await;
}

// ── Compression Pipeline ───────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn start_compression_task(
    api: &Bot,
    chat_id: i64,
    prompt_msg_id: i32,
    user_id: i64,
    config: CompressConfig,
    files: Vec<CompressFileEntry>,
    trace_id: u64,
    database: &Option<PostgresDatabase>,
    flow_manager: &FlowManager,
) {
    if crate::moebius::cpu::is_user_cpu_busy(user_id).await {
        let _ = crate::bot::send_text_md(api, chat_id, &t("active_job_running")).await;
        return;
    }

    // Remove reply keyboard first
    let remove_kb = frankenstein::types::ReplyKeyboardRemove::builder()
        .remove_keyboard(true)
        .build();
    let _ = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text("⏳")
                .reply_markup(ReplyMarkup::ReplyKeyboardRemove(remove_kb))
                .build(),
        )
        .await;

    // Quota Paywall Check
    let Some(db) = database.as_ref() else {
        crate::log_ev!("filecompress", trace_id, "db_connect_failed", "err" => "no_db");
        let _ = send_text_with_back(api, chat_id, &t("fc.error.compress_failed")).await;
        return;
    };
    let db_client = db.client();

    let rank = rank::effective_rank(db_client, user_id).await;
    let daily_limit = rank.compress_cpu_daily_secs();
    let monthly_limit = rank.compress_cpu_monthly_secs();

    let next_rank = match rank {
        crate::rank::types::Rank::Dalavar => Some(crate::rank::types::Rank::Sepahbod),
        crate::rank::types::Rank::Sepahbod | crate::rank::types::Rank::Esfandyar => {
            Some(crate::rank::types::Rank::Sohrab)
        }
        crate::rank::types::Rank::Sohrab => Some(crate::rank::types::Rank::Rostam),
        crate::rank::types::Rank::Rostam => None,
    };

    // Quota unit is CPU seconds, unknown until job finishes. Reserve 1 second
    // so check + debit remains atomic, then settle remainder with add_usage after completion.
    let mut reserved = false;
    for (kind, window, limit, label_key, event) in [
        (
            QuotaKind::CompressCpuDaily,
            86400i64,
            daily_limit,
            "fc.error.quota_daily",
            "paywall_daily_blocked",
        ),
        (
            QuotaKind::CompressCpuMonthly,
            2592000i64,
            monthly_limit,
            "fc.error.quota_monthly",
            "paywall_monthly_blocked",
        ),
    ] {
        match rank::quota::reserve_usage(db_client, user_id, kind, 1, window, limit as i64).await {
            Ok(Some(used)) => {
                crate::log_ev!("filecompress", trace_id, "quota_reserved", "kind" => kind.as_str(), "used" => used, "limit" => limit);
            }
            Ok(None) => {
                crate::log_ev!("filecompress", trace_id, event, "limit" => limit, "=>" => "blocked");
                if reserved {
                    let _ = rank::quota::refund_usage(
                        db_client,
                        user_id,
                        QuotaKind::CompressCpuDaily,
                        1,
                        86400,
                    )
                    .await;
                }
                let label = t(label_key);
                if let Some(nr) = next_rank {
                    crate::rank::paywall::block_limit(api, chat_id, &label, nr).await;
                } else {
                    let _ = send_text_with_back(api, chat_id, &label).await;
                }
                return;
            }
            Err(e) => {
                // fail closed — notify user on DB error
                crate::log_ev!("filecompress", trace_id, "quota_reserve", "err" => format!("{e}"), "=>" => "fail");
                if reserved {
                    let _ = rank::quota::refund_usage(
                        db_client,
                        user_id,
                        QuotaKind::CompressCpuDaily,
                        1,
                        86400,
                    )
                    .await;
                }
                crate::rank::paywall::quota_db_error(api, chat_id, "filecompress", &format!("{e}"))
                    .await;
                return;
            }
        }
        reserved = true;
    }

    let progress = Arc::new(super::progress::JobProgress::new(files.len()));
    progress.set_downloading(1);

    let progress_msg = match api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&apply_premium_to_md(&render_progress(&progress, 0)))
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(job_cancel_keyboard()))
                .build(),
        )
        .await
    {
        Ok(m) => m.result.message_id,
        Err(_) => prompt_msg_id,
    };

    // Cancel flag + staged progress ticker on the status message
    let cancel_flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut jobs) = ACTIVE_FC_JOBS.lock() {
        jobs.insert(user_id, cancel_flag.clone());
    }
    let timer_running = Arc::new(AtomicBool::new(true));
    let timer_flag = timer_running.clone();
    let timer_cancel = cancel_flag.clone();
    let timer_progress = progress.clone();
    let api_timer = api.clone();
    let timer_handle = crate::app::spawn_user_task(async move {
        let started = std::time::Instant::now();
        let mut last = String::new();
        while timer_flag.load(Ordering::Relaxed) && !timer_cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if !timer_flag.load(Ordering::Relaxed) || timer_cancel.load(Ordering::Relaxed) {
                break;
            }
            let text = apply_premium_to_md(&render_progress(
                &timer_progress,
                started.elapsed().as_secs(),
            ));
            // Telegram rejects an edit that changes nothing; skip the round-trip.
            if text == last {
                continue;
            }
            last = text.clone();
            let params = EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(progress_msg)
                .text(&text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(job_cancel_keyboard())
                .build();
            let _ = api_timer.edit_message_text(&params).await;
        }
    });

    let api_clone = api.clone();
    let db_clone = database.clone();
    let fm = flow_manager.clone();
    crate::app::spawn_user_task(async move {
        run_filecompress_worker(
            api_clone,
            chat_id,
            progress_msg,
            user_id,
            config,
            files,
            trace_id,
            db_clone,
            fm,
            cancel_flag,
            timer_running,
            timer_handle,
            progress,
        )
        .await;
    });
}

/// Renders the status message for whichever stage is running. `elapsed` is the
/// whole job's wall time; the ETA uses only the compression part of it.
fn render_progress(progress: &super::progress::JobProgress, elapsed: u64) -> String {
    use super::progress::{STAGE_DOWNLOAD, bar, eta_secs};
    if progress.stage() == STAGE_DOWNLOAD {
        return t("fc.downloading")
            .replace("{idx}", &progress.file_idx().to_string())
            .replace("{total}", &progress.file_total().to_string())
            .replace("{elapsed}", &format_clock(elapsed));
    }
    let pct = progress.percent();
    let compress_elapsed = elapsed.saturating_sub(progress.compress_offset());
    match eta_secs(pct, compress_elapsed) {
        Some(eta) => t("fc.processing_eta")
            .replace("{bar}", &bar(pct))
            .replace("{percent}", &pct.to_string())
            .replace("{elapsed}", &format_clock(elapsed))
            .replace("{eta}", &format_clock(eta)),
        None => t("fc.processing")
            .replace("{bar}", &bar(pct))
            .replace("{percent}", &pct.to_string())
            .replace("{elapsed}", &format_clock(elapsed)),
    }
}

// Test-only access to real keyboards
#[cfg(feature = "testapi")]
pub fn options_keyboard_for_test(config: &CompressConfig) -> InlineKeyboardMarkup {
    options_keyboard(config)
}

#[cfg(feature = "testapi")]
pub fn cancel_only_keyboard_for_test() -> InlineKeyboardMarkup {
    cancel_only_keyboard()
}

#[cfg(feature = "testapi")]
pub fn job_cancel_keyboard_for_test() -> InlineKeyboardMarkup {
    job_cancel_keyboard()
}

#[cfg(feature = "testapi")]
pub fn render_progress_for_test(progress: &super::progress::JobProgress, elapsed: u64) -> String {
    render_progress(progress, elapsed)
}

#[cfg(feature = "testapi")]
pub fn format_clock_for_test(secs: u64) -> String {
    format_clock(secs)
}

/// mm:ss (or hh:mm:ss) for elapsed time display.
fn format_clock(secs: u64) -> String {
    if secs >= 3600 {
        format!(
            "{:02}:{:02}:{:02}",
            secs / 3600,
            (secs % 3600) / 60,
            secs % 60
        )
    } else {
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_filecompress_worker(
    api: Bot,
    chat_id: i64,
    progress_msg_id: i32,
    user_id: i64,
    config: CompressConfig,
    files: Vec<CompressFileEntry>,
    trace_id: u64,
    database: Option<PostgresDatabase>,
    flow_manager: FlowManager,
    cancel_flag: Arc<AtomicBool>,
    timer_running: Arc<AtomicBool>,
    timer_handle: tokio::task::JoinHandle<()>,
    progress: Arc<super::progress::JobProgress>,
) {
    let job_started = std::time::Instant::now();
    // Stop ticker + release cancel flag on exit
    macro_rules! stop_timer {
        () => {{
            timer_running.store(false, Ordering::Relaxed);
            remove_active_fc_job(user_id);
        }};
    }

    // Called only when database is present and both windows are reserved
    macro_rules! refund {
        ($why:expr) => {
            if let Some(db) = database.as_ref() {
                crate::log_ev!("filecompress", trace_id, "quota_refund", "why" => $why);
                for (kind, window) in [
                    (QuotaKind::CompressCpuDaily, 86400i64),
                    (QuotaKind::CompressCpuMonthly, 2592000i64),
                ] {
                    if let Err(e) =
                        rank::quota::refund_usage(db.client(), user_id, kind, 1, window).await
                    {
                        crate::log_ev!("filecompress", trace_id, "quota_refund", "err" => format!("{e}"), "=>" => "fail");
                        crate::stats::record_error_global("filecompress", "quota_refund_failed")
                            .await;
                    }
                }
            }
        };
    }

    let work_dir = std::env::temp_dir().join(format!("filecompress_{trace_id}"));
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        crate::log_ev!("filecompress", trace_id, "mkdir_failed", "err" => format!("{e}"));
        stop_timer!();
        refund!("mkdir_failed");
        let _ = send_text_with_back(&api, chat_id, &t("fc.error.compress_failed")).await;
        return;
    }

    let mut local_input_paths = Vec::new();

    for (idx, entry) in files.iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            crate::log_ev!("filecompress", trace_id, "cancelled_during_download", "idx" => idx);
            stop_timer!();
            refund!("cancelled");
            std::fs::remove_dir_all(&work_dir).ok();
            return;
        }
        let local_path = work_dir.join(&entry.filename);
        progress.set_downloading(idx + 1);
        crate::log_ev!("filecompress", trace_id, "downloading_file", "idx" => idx, "name" => &entry.filename);
        if let Err(e) = download_telegram_file(&api, &entry.file_id, &local_path).await {
            crate::log_ev!("filecompress", trace_id, "download_failed", "err" => format!("{e}"));
            stop_timer!();
            refund!("download_failed");
            std::fs::remove_dir_all(&work_dir).ok();
            let _ = send_text_with_back(&api, chat_id, &t("fc.error.download_failed")).await;
            return;
        }
        let size = std::fs::metadata(&local_path).map(|m| m.len()).unwrap_or(0);
        crate::log_ev!("filecompress", trace_id, "download_done", "idx" => idx, "bytes" => size, "=>" => "ok");
        local_input_paths.push(local_path);
    }

    if cancel_flag.load(Ordering::Relaxed) {
        crate::log_ev!("filecompress", trace_id, "cancelled_before_engine", "user_id" => user_id);
        stop_timer!();
        refund!("cancelled");
        std::fs::remove_dir_all(&work_dir).ok();
        return;
    }

    let cores = acquire_cpu(user_id, trace_id).await;
    let timeout = Duration::from_secs(1800); // 30 minutes max

    // Downloads and the broker queue are behind us; the ETA clock starts here.
    progress.set_compressing(job_started.elapsed().as_secs());
    crate::log_ev!("filecompress", trace_id, "engine_start", "files" => local_input_paths.len());
    let compress_res = run_compress(
        &work_dir,
        &config,
        &local_input_paths,
        timeout,
        &cores,
        trace_id,
        &cancel_flag,
        &progress,
    )
    .await;

    release_cpu(cores, trace_id).await;
    stop_timer!();
    let _ = timer_handle.await;

    // User clicked cancel mid-job: discard output and refund quota.
    if cancel_flag.load(Ordering::Relaxed) {
        crate::log_ev!("filecompress", trace_id, "cancelled_mid_job", "user_id" => user_id);
        refund!("cancelled");
        std::fs::remove_dir_all(&work_dir).ok();
        return;
    }

    let result = match compress_res {
        Ok(r) => r,
        Err(CompressError::Timeout) => {
            crate::log_ev!("filecompress", trace_id, "timeout");
            refund!("timeout");
            std::fs::remove_dir_all(&work_dir).ok();
            let _ = send_text_with_back(&api, chat_id, &t("fc.error.timeout")).await;
            return;
        }
        Err(e) => {
            crate::log_ev!("filecompress", trace_id, "compress_failed", "err" => format!("{e}"));
            refund!("compress_failed");
            std::fs::remove_dir_all(&work_dir).ok();
            let _ = send_text_with_back(&api, chat_id, &t("fc.error.compress_failed")).await;
            return;
        }
    };

    // Settlement: 1 second deducted during reservation, settle remainder with add_usage.
    let cpu_secs_used = result.cpu_secs.ceil() as i64;
    let cpu_secs_delta = (cpu_secs_used - 1).max(0);
    if let Some(db) = database.as_ref() {
        let client = db.client();
        if cpu_secs_delta > 0 {
            for (kind, window) in [
                (QuotaKind::CompressCpuDaily, 86400i64),
                (QuotaKind::CompressCpuMonthly, 2592000i64),
            ] {
                if let Err(e) =
                    rank::quota::add_usage(client, user_id, kind, cpu_secs_delta, window).await
                {
                    crate::log_ev!("filecompress", trace_id, "quota_settle", "kind" => kind.as_str(), "err" => format!("{e}"), "=>" => "fail");
                    crate::stats::record_error_global("filecompress", "quota_settle_failed").await;
                }
            }
        }
        crate::log_ev!("filecompress", trace_id, "quota_settled", "cpu_secs" => cpu_secs_used);

        crate::stats::record_event_user(
            user_id,
            "filecompress",
            config.fmt.as_str(),
            "ok",
            result.output_total_bytes as i64,
        )
        .await;
    }

    let input_fmt = fmt_bytes(result.input_total_bytes);
    let output_fmt = fmt_bytes(result.output_total_bytes);
    let reduction_pct = if result.input_total_bytes > 0 {
        ((1.0 - (result.output_total_bytes as f64 / result.input_total_bytes as f64)) * 100.0)
            .max(0.0)
    } else {
        0.0
    };

    let caption = apply_premium_to_md(&format!(
        "{}\n\n{}",
        t("fc.result_caption"),
        t("fc.result_report")
            .replace("{before}", &escape_md(&input_fmt))
            .replace("{after}", &escape_md(&output_fmt))
            .replace("{percent}", &escape_md(&format!("{reduction_pct:.1}")))
            .replace(
                "{cpu_time}",
                &escape_md(&format!("{:.1}s", result.cpu_secs))
            )
    ));

    let part_count = result.output_paths.len();

    for (idx, path) in result.output_paths.iter().enumerate() {
        let part_caption = if part_count > 1 {
            format!(
                "{}\n{}",
                caption,
                t("fc.result_part_caption")
                    .replace("{part}", &(idx + 1).to_string())
                    .replace("{total}", &part_count.to_string())
            )
        } else {
            caption.clone()
        };

        let params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(PathBuf::from(path))
            .caption(&part_caption)
            .parse_mode(ParseMode::MarkdownV2)
            .build();

        if let Err(e) = api.send_document(&params).await {
            crate::log_ev!("filecompress", trace_id, "send_failed", "err" => format!("{e}"));
            let _ = send_text_with_back(&api, chat_id, &t("fc.error.send_failed")).await;
            break;
        }
    }

    // Delete progress message
    let _ = api
        .delete_message(
            &DeleteMessageParams::builder()
                .chat_id(chat_id)
                .message_id(progress_msg_id)
                .build(),
        )
        .await;

    std::fs::remove_dir_all(&work_dir).ok();

    // Re-arm flow with same config so user can send next batch immediately
    let upload_text = apply_premium_to_md(&t("fc.upload_prompt").replace("{count}", "0"));
    let send_res = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&upload_text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::ReplyKeyboardMarkup(done_reply_keyboard()))
                .build(),
        )
        .await;
    if let Ok(m) = send_res {
        flow_manager.set(
            user_id,
            FlowState::AwaitingCompressFiles {
                config: Box::new(config),
                files: Vec::new(),
                prompt_msg_id: m.result.message_id,
            },
        );
    }
}


async fn acquire_cpu(user_id: i64, trace_id: u64) -> Vec<i32> {
    let client = crate::http::client();
    let res = client
        .post(format!("{SEP_BASE}/cpu/acquire"))
        .form(&[
            ("user_id", user_id.to_string()),
            ("is_vip", "false".to_string()),
        ])
        .timeout(Duration::from_secs(120))
        .send()
        .await;
    match res {
        Ok(r) => {
            let json: serde_json::Value = r.json().await.unwrap_or_default();
            let cores: Vec<i32> = json
                .get("cores")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            crate::log_ev!("filecompress", trace_id, "cpu_acquired", "cores" => format!("{cores:?}"));
            cores
        }
        Err(e) => {
            crate::log_ev!("filecompress", trace_id, "cpu_acquire_failed", "err" => format!("{e}"));
            vec![]
        }
    }
}

async fn release_cpu(cores: Vec<i32>, trace_id: u64) {
    if cores.is_empty() {
        return;
    }
    let client = crate::http::client();
    let body = serde_json::json!({ "cores": cores });
    let r = client
        .post(format!("{SEP_BASE}/cpu/release"))
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    crate::log_ev!("filecompress", trace_id, "cpu_released", "cores" => format!("{cores:?}"), "=>" => if r.is_ok() { "ok" } else { "fail" });
}

fn fmt_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.2} GB", mb / 1024.0)
    } else {
        format!("{mb:.1} MB")
    }
}

fn escape_md(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '*' | '\\' | '_' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '='
            | '|' | '{' | '}' | '.' | '!' => format!("\\{c}"),
            other => other.to_string(),
        })
        .collect()
}
