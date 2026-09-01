//! Session state management, menu rendering, password prompt, and media file intake.

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{DeleteMessageParams, EditMessageTextParams, SendMessageParams},
    types::{Message, ReplyMarkup, ReplyParameters},
};

use super::config::{CompressAlgo, CompressConfig, CompressFmt};
use super::pipeline::start_compression_task;
use super::progress::{cancel_only_keyboard, done_inline_keyboard, options_keyboard};
use crate::bot::send_text_with_back;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::flow::CompressFileEntry;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t, tf};
use crate::log::next_trace_id;

/// Callback response; populated text = transient toast on user screen.
pub async fn fc_answer(api: &Bot, cb_id: &str, text: Option<String>) {
    let b = frankenstein::methods::AnswerCallbackQueryParams::builder().callback_query_id(cb_id);
    let _ = match text {
        Some(txt) => api.answer_callback_query(&b.text(txt).build()).await,
        None => api.answer_callback_query(&b.build()).await,
    };
}

pub async fn show_options_menu(api: &Bot, chat_id: i64, message_id: i32, config: &CompressConfig) {
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
    if let Err(ref e) = api.edit_message_text(&params).await {
        let desc = format!("{e:?}");
        if !desc.contains("message is not modified") {
            crate::log_ev!("filecompress", 0, "show_options_menu_err", "err" => desc);
        }
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

pub async fn handle_options_action(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
    action: &str,
    cb_id: &str,
) {
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
                            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(done_inline_keyboard()))
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
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(done_inline_keyboard()))
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
                        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(done_inline_keyboard()))
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
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(done_inline_keyboard()))
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
