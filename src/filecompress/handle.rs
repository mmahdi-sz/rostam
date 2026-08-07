use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{DeleteMessageParams, EditMessageTextParams, SendDocumentParams, SendMessageParams},
    types::{
        InlineKeyboardMarkup, KeyboardButton, Message, ReplyKeyboardMarkup, ReplyMarkup,
        ReplyParameters,
    },
};

use super::config::{CompressAlgo, CompressConfig, CompressFmt};
use super::engine::{CompressError, run_compress};
use crate::bot::{download_telegram_file, edit_to_tools, send_text_with_back};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::flow::CompressFileEntry;
use crate::emoji::panel::{btn_icon, btn_icon_danger, btn_icon_plain, btn_icon_success};
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t};
use crate::log::next_trace_id;
use crate::rank::{self, quota::QuotaKind};

const SEP_BASE: &str = "http://127.0.0.1:6589";

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
    rows.push(vec![zip_btn, sz_btn, rar_btn]);

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

    // Row 5: Password Encryption Toggle
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

    // Row 6: Split into parts Toggle
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

    // Row 9: Solid Mode Toggle (BLUE for "کل پوشه: سریع تر" per user request!)
    let (solid_label, solid_btn) = if config.solid {
        (
            t("fc.toggle_solid"),
            btn_icon(&t("fc.solid_mode_solid"), "fc:toggle:solid", "pack_folder"),
        )
    } else {
        (
            t("fc.toggle_solid"),
            btn_icon(&t("fc.solid_mode_normal"), "fc:toggle:solid", "rocket"),
        )
    };
    rows.push(vec![
        btn_icon(&solid_label, "fc:toggle:solid", "pack_folder"),
        solid_btn,
    ]);

    // Row 10: Confirm + Cancel
    rows.push(vec![
        btn_icon_success(&t("fc.confirm_button"), "fc:confirm", "confirm"),
        btn_icon_plain(&t("start.back"), CB_FC_CANCEL, "back"),
    ]);

    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

fn done_reply_keyboard() -> ReplyKeyboardMarkup {
    ReplyKeyboardMarkup::builder()
        .keyboard(vec![vec![
            KeyboardButton::builder()
                .text(t("fc.done_upload_button"))
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

pub async fn handle_fc_callback(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
    action: &str,
    _database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    crate::log_ev!("filecompress", trace_id, "callback", "action" => action, "user_id" => user_id);

    if action == "cancel" {
        flow_manager.clear(user_id);
        let _ = edit_to_tools(api, chat_id, message_id).await;
        return;
    }

    if action == "noop" {
        return;
    }

    let state = flow_manager.get(user_id);

    match action {
        "fmt:zip" | "fmt:7z" | "fmt:rar" => {
            let fmt_str = &action["fmt:".len()..];
            let fmt = CompressFmt::from_str(fmt_str).unwrap_or(CompressFmt::SevenZ);
            let mut config = match state {
                FlowState::AwaitingCompressOptions { config } => config,
                _ => CompressConfig::default(),
            };
            config.fmt = fmt;
            if config.fmt == CompressFmt::Rar && config.level > 5 {
                config.level = 5;
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
            let max_level = if config.fmt == CompressFmt::Rar { 5 } else { 9 };
            if action == "lvl:up" && config.level < max_level {
                config.level += 1;
            } else if action == "lvl:down" && config.level > 0 {
                config.level -= 1;
            }
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
                    .callback_query_id(action)
                    .text(t("fc.error.obfuscate_needs_password"))
                    .show_alert(true)
                    .build();
                let _ = api.answer_callback_query(&params).await;
                return;
            }
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
    let key = if config.fmt == CompressFmt::SevenZ {
        "fc.welcome_7z"
    } else {
        "fc.welcome"
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
    let _ = api.send_message(&reply_params).await;

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
        let _ = api.edit_message_text(&params).await;
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
    )
    .await;
}

// ── Compression Pipeline ───────────────────────────────────────────────────────

async fn start_compression_task(
    api: &Bot,
    chat_id: i64,
    prompt_msg_id: i32,
    user_id: i64,
    config: CompressConfig,
    files: Vec<CompressFileEntry>,
    trace_id: u64,
    database: &Option<PostgresDatabase>,
) {
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

    // اینجا واحد سهمیه ثانیه‌ی CPU است و مقدارش تا پایان کار معلوم نیست، پس
    // مثل بقیه نمی‌شود مقدار واقعی را رزرو کرد. یک ثانیه رزرو می‌شود تا
    // «چک + کسر» یک statement بماند (دقیقاً معادل `used >= limit` قبلی)، و
    // بعد از کار با `add_usage` مابقی تسویه می‌شود.
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
                // fail closed — تصمیم کاربر: در خطای دیتابیس کاربر مطلع شود.
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

    let progress_msg = match api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&apply_premium_to_md(
                    &t("fc.processing")
                        .replace("{bar}", "░░░░░░░░░░")
                        .replace("{percent}", "0")
                        .replace("{elapsed}", "0s"),
                ))
                .parse_mode(ParseMode::MarkdownV2)
                .build(),
        )
        .await
    {
        Ok(m) => m.result.message_id,
        Err(_) => prompt_msg_id,
    };

    let api_clone = api.clone();
    let db_clone = database.clone();
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
        )
        .await;
    });
}

async fn run_filecompress_worker(
    api: Bot,
    chat_id: i64,
    progress_msg_id: i32,
    user_id: i64,
    config: CompressConfig,
    files: Vec<CompressFileEntry>,
    trace_id: u64,
    database: Option<PostgresDatabase>,
) {
    // ورود به این تابع فقط وقتی رخ می‌دهد که `database` موجود بوده و هر دو
    // پنجره رزرو شده‌اند، پس `database.is_some()` همان پرچم reserved است.
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
        refund!("mkdir_failed");
        let _ = send_text_with_back(&api, chat_id, &t("fc.error.compress_failed")).await;
        return;
    }

    let mut local_input_paths = Vec::new();

    for (idx, entry) in files.iter().enumerate() {
        let local_path = work_dir.join(&entry.filename);
        crate::log_ev!("filecompress", trace_id, "downloading_file", "idx" => idx, "name" => &entry.filename);
        if let Err(e) = download_telegram_file(&api, &entry.file_id, &local_path).await {
            crate::log_ev!("filecompress", trace_id, "download_failed", "err" => format!("{e}"));
            refund!("download_failed");
            std::fs::remove_dir_all(&work_dir).ok();
            let _ = send_text_with_back(&api, chat_id, &t("fc.error.download_failed")).await;
            return;
        }
        local_input_paths.push(local_path);
    }

    let cores = acquire_cpu(user_id, trace_id).await;
    let timeout = Duration::from_secs(1800); // 30 minutes max

    crate::log_ev!("filecompress", trace_id, "engine_start", "files" => local_input_paths.len());
    let compress_res = run_compress(
        &work_dir,
        &config,
        &local_input_paths,
        timeout,
        &cores,
        trace_id,
    )
    .await;

    release_cpu(cores, trace_id).await;

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

    // تسویه: هنگام رزرو ۱ ثانیه کسر شد، پس اینجا فقط مابقی افزوده می‌شود.
    // این تنها هندلری است که `add_usage` در آن می‌ماند، چون واحد سهمیه
    // (ثانیه‌ی CPU) پیش از پایان کار قابل اندازه‌گیری نیست.
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
}

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

async fn acquire_cpu(user_id: i64, trace_id: u64) -> Vec<i32> {
    let client = HTTP_CLIENT.get_or_init(reqwest::Client::new);
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
    let client = HTTP_CLIENT.get_or_init(reqwest::Client::new);
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
