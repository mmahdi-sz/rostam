use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

use frankenstein::AsyncTelegramApi;
use frankenstein::client_reqwest::Bot;
use frankenstein::types::Message;

use crate::config;
use crate::cookie_pool::{CookiePool, CookieSource};
use crate::database::postgresql::PostgresDatabase;
use crate::denoise;
use crate::emoji::{FlowManager, FlowState, handler as emoji_handler};
use crate::gemini_watermark::handle_gwm_image;
use crate::i18n::{t, tf};
use crate::ip_lookup::handle_ip_lookup_text;
use crate::log::next_trace_id;
use crate::pdfcompress::handle_pdf_file;
use crate::separation::handle_separation_audio;
use crate::stt::handle::handle_stt_audio;
use crate::surge_dl::{handle_surge_rename_text, handle_surge_text};
use crate::upscale::handle_upscale_image;
use crate::youtube::trace::log_trace;
use crate::youtube::{extract_youtube_urls, handle_youtube_url};

pub(super) async fn handle_flow_message(
    api: &Bot,
    cookie_pool: &Arc<Mutex<CookiePool>>,
    database: &Option<PostgresDatabase>,
    flow_manager: &mut FlowManager,
    rate_limit_tx: &UnboundedSender<CookieSource>,
    message: &Message,
    uid: i64,
) -> crate::error::Result<bool> {
    if matches!(flow_manager.get(uid), FlowState::Idle) {
        return Ok(false);
    }

    // Admin sent code generation args (e.g. `30d es 1u`)
    if matches!(flow_manager.get(uid), FlowState::AwaitingRedeemGenArgs) {
        if let Some(text) = message.text.as_deref() {
            flow_manager.clear(uid);
            let is_admin = config::admin_user_id().map(|id| id == uid).unwrap_or(false);
            if is_admin {
                crate::redeem::handle::handle_generate(api, message.chat.id, uid, text, database)
                    .await;
            }
            return Ok(true);
        }
    }

    // Admin sent new lock link
    if matches!(flow_manager.get(uid), FlowState::AwaitingForceJoinLink) {
        if let Some(text) = message.text.as_deref() {
            let is_admin = config::admin_user_id().map(|id| id == uid).unwrap_or(false);
            if is_admin {
                crate::force_join::handle_link_message(
                    api,
                    message.chat.id,
                    text,
                    flow_manager,
                    uid,
                )
                .await;
            }
            return Ok(true);
        }
    }

    // Admin sent username/numeric ID/forward for private link
    if let FlowState::AwaitingForceJoinPrivateInfo { link } = flow_manager.get(uid) {
        let is_admin = config::admin_user_id().map(|id| id == uid).unwrap_or(false);
        if is_admin {
            crate::force_join::handle_private_info_message(
                api,
                message.chat.id,
                &link,
                &message,
                flow_manager,
                uid,
            )
            .await;
        }
        return Ok(true);
    }

    // Admin sent input for lock field wizard (name/time/member/reserve)
    if let FlowState::AwaitingForceJoinField { lock_id, field, .. } = flow_manager.get(uid) {
        if let Some(text) = message.text.as_deref() {
            let is_admin = config::admin_user_id().map(|id| id == uid).unwrap_or(false);
            if is_admin {
                crate::force_join::handle_field_message(
                    api,
                    message.chat.id,
                    lock_id,
                    &field,
                    text,
                    flow_manager,
                    uid,
                    database,
                )
                .await;
            }
            return Ok(true);
        }
    }

    // Admin sent broadcast banner (any msg/media/text)
    if let FlowState::AwaitingBroadcastBanner { mode, pin } = flow_manager.get(uid) {
        let is_admin = config::admin_user_id().map(|id| id == uid).unwrap_or(false);
        if is_admin {
            let banner_chat_id = message.chat.id;
            let banner_message_id = message.message_id;

            let (total_users, active_users) = if let Some(db) = database {
                if let Ok(client) = db.get().await {
                    match crate::stats::get_broadcast_user_counts(&client).await {
                        Ok(counts) => (counts.total, counts.active),
                        Err(_) => (1, 1),
                    }
                } else {
                    (1, 1)
                }
            } else {
                (1, 1)
            };

            flow_manager.set(
                uid,
                FlowState::AwaitingBroadcastTarget {
                    mode,
                    pin,
                    banner_chat_id,
                    banner_message_id,
                    total_users,
                    active_users,
                },
            );

            let prompt_text = crate::i18n::tf(
                "admin.broadcast.target_prompt",
                &[
                    ("total", &total_users.to_string()),
                    ("active", &active_users.to_string()),
                ],
            );
            let kb = crate::admin::broadcast::broadcast_target_keyboard(active_users, total_users);

            let _ = crate::bot::send_text_with_kb(api, message.chat.id, &prompt_text, kb).await;
        }
        return Ok(true);
    }

    // Admin sent broadcast limit count (e.g. 500)
    if let FlowState::AwaitingBroadcastTarget {
        mode,
        pin,
        banner_chat_id,
        banner_message_id,
        ..
    } = flow_manager.get(uid)
    {
        if let Some(text) = message.text.as_deref() {
            let is_admin = config::admin_user_id().map(|id| id == uid).unwrap_or(false);
            if is_admin {
                if let Ok(limit_num) = text.trim().parse::<i64>() {
                    flow_manager.clear(uid);
                    crate::admin::broadcast::spawn_broadcast_job(
                        api.clone(),
                        database.clone(),
                        message.chat.id,
                        mode,
                        pin,
                        banner_chat_id,
                        banner_message_id,
                        false,
                        Some(limit_num),
                    );
                }
            }
            return Ok(true);
        }
    }

    if emoji_handler::handle_emoji_flow_message(api, &message, uid, flow_manager, database).await {
        return Ok(true);
    }

    if let FlowState::AwaitingSttAudio { config } = flow_manager.get(uid) {
        if message.voice.is_some() || message.audio.is_some() || message.document.is_some() {
            let file_id = message
                .voice
                .as_ref()
                .map(|v| v.file_id.clone())
                .or_else(|| message.audio.as_ref().map(|a| a.file_id.clone()))
                .or_else(|| message.document.as_ref().map(|d| d.file_id.clone()));
            if let Some(fid) = file_id {
                let trace_id = next_trace_id();
                log_trace(
                    trace_id,
                    "stt_route_dispatched",
                    &format!("user_id={uid} chat_id={}", message.chat.id),
                );
                let api2 = api.clone();
                let chat_id2 = message.chat.id;
                let db2 = database.clone();
                // Flow deliberately kept: user can send next voice with same model after result
                // Spawn so the event loop stays free during STT (minutes-long operation)
                crate::app::spawn_user_task(async move {
                    handle_stt_audio(&api2, chat_id2, &fid, uid, &config, db2).await;
                });
            }
            return Ok(true);
        }
    }

    if matches!(flow_manager.get(uid), FlowState::AwaitingDenoiseAudio) {
        if message.voice.is_some()
            || message.audio.is_some()
            || message.video.is_some()
            || message.document.is_some()
        {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "denoise_route_dispatched",
                &format!("user_id={uid} chat_id={}", message.chat.id),
            );
            // Spawn so the event loop stays free during denoise (minutes-long operation).
            flow_manager.clear(uid);
            let api2 = api.clone();
            let msg2 = message.clone();
            let db2 = database.clone();
            crate::app::spawn_user_task(async move {
                denoise::handle_denoise_audio(&api2, &msg2, uid, &db2).await;
            });
            return Ok(true);
        }
    }

    if let FlowState::AwaitingUpscaleImage {
        scale_factor,
        model_name,
        ..
    } = flow_manager.get(uid)
    {
        if message.photo.is_some() || message.document.is_some() {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "upscale_route_dispatched",
                &format!("user_id={uid} model={model_name}"),
            );
            flow_manager.clear(uid);
            let api2 = api.clone();
            let msg2 = message.clone();
            let db2 = database.clone();
            crate::app::spawn_user_task(async move {
                handle_upscale_image(api2, msg2, uid, scale_factor, model_name, db2).await;
            });
            return Ok(true);
        }
    }

    if matches!(flow_manager.get(uid), FlowState::AwaitingSeparation) {
        if message.audio.is_some()
            || message.voice.is_some()
            || message.document.is_some()
            || message.video.is_some()
        {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "separation_route_dispatched",
                &format!("user_id={uid} chat_id={}", message.chat.id),
            );
            handle_separation_audio(api, &message, uid, flow_manager).await;
            return Ok(true);
        }
    }

    if matches!(flow_manager.get(uid), FlowState::AwaitingGeminiWmImage) {
        if message.photo.is_some() || message.document.is_some() {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "gwm_route_dispatched",
                &format!("user_id={uid} chat_id={}", message.chat.id),
            );
            // Spawn so the event loop stays free during watermark removal.
            flow_manager.clear(uid);
            let api2 = api.clone();
            let msg2 = message.clone();
            crate::app::spawn_user_task(async move {
                handle_gwm_image(&api2, &msg2, uid).await;
            });
            return Ok(true);
        }
    }

    if matches!(flow_manager.get(uid), FlowState::AwaitingDeoldifyImage) {
        if message.photo.is_some() || message.document.is_some() {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "deoldify_route_dispatched",
                &format!("user_id={uid} chat_id={}", message.chat.id),
            );
            flow_manager.clear(uid);
            let api2 = api.clone();
            let msg2 = message.clone();
            let fm_clone = flow_manager.clone();
            let db_clone = database.clone();
            crate::app::spawn_user_task(async move {
                crate::deoldify::handle_deoldify_image(&api2, &msg2, uid, &fm_clone, db_clone)
                    .await;
            });
            return Ok(true);
        }
    }

    if matches!(flow_manager.get(uid), FlowState::AwaitingNobgImage) {
        if message.photo.is_some() || message.document.is_some() {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "nobg_route_dispatched",
                &format!("user_id={uid} chat_id={}", message.chat.id),
            );
            let api2 = api.clone();
            let msg2 = message.clone();
            let fm = flow_manager.clone();
            let db = database.clone();
            crate::app::spawn_user_task(async move {
                crate::feynobg::handle_nobg_image(&api2, &msg2, uid, &fm, db).await;
            });
            return Ok(true);
        }
    }

    if matches!(flow_manager.get(uid), FlowState::AwaitingTtsText) {
        if let Some(text) = &message.text {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "tts_text_dispatched",
                &format!("user_id={uid} chat_id={}", message.chat.id),
            );
            let text_clone = text.clone();
            let api2 = api.clone();
            let chat_id = message.chat.id;
            let flow_manager_clone = flow_manager.clone();
            let database_clone = database.clone();
            flow_manager.clear(uid);
            crate::app::spawn_user_task(async move {
                crate::moss_tts::handle_tts_text(
                    &api2,
                    chat_id,
                    uid,
                    &text_clone,
                    &flow_manager_clone,
                    database_clone,
                )
                .await;
            });
            return Ok(true);
        }
    }

    if matches!(flow_manager.get(uid), FlowState::AwaitingPdfCompressFile) {
        if message.document.is_some() {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "pdfcompress_route_dispatched",
                &format!("user_id={uid} chat_id={}", message.chat.id),
            );
            handle_pdf_file(api, &message, uid, flow_manager).await;
        } else {
            let _ =
                crate::bot::send_text(api, message.chat.id, &t("pdfcompress.busy_warning")).await;
        }
        return Ok(true);
    }

    if matches!(
        flow_manager.get(uid),
        FlowState::AwaitingPdfCompressLevel { .. }
    ) {
        let _ = crate::bot::send_text(api, message.chat.id, &t("pdfcompress.busy_warning")).await;
        return Ok(true);
    }

    if matches!(flow_manager.get(uid), FlowState::AwaitingPkgFile) {
        if message.document.is_some() {
            let trace_id = next_trace_id();
            log_ev!("pkgconvert", trace_id, "file_dispatched", "user_id" => uid);
            crate::pkgconvert::handle_pkg_file(api, &message, uid, flow_manager, database).await;
        } else {
            let _ = crate::bot::send_text_md(api, message.chat.id, &t("pkg.prompt")).await;
        }
        return Ok(true);
    }

    if matches!(
        flow_manager.get(uid),
        FlowState::AwaitingPkgConvertChoice { .. }
    ) {
        return Ok(true);
    }

    if matches!(flow_manager.get(uid), FlowState::AwaitingIpLookupInput) {
        if message.text.is_some() {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "ip_lookup_route_dispatched",
                &format!("user_id={uid} chat_id={}", message.chat.id),
            );
            handle_ip_lookup_text(api, &message, uid, flow_manager).await;
        }
        return Ok(true);
    }

    if matches!(flow_manager.get(uid), FlowState::AwaitingSurgeUrlInput) {
        if let Some(txt) = message.text.as_deref() {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "surge_dl_route_dispatched",
                &format!("user_id={uid} chat_id={}", message.chat.id),
            );
            flow_manager.clear(uid);
            if crate::musicset::try_route_set(api, message.chat.id, uid, database, txt) {
                return Ok(true);
            }
            if let Some(track_id) = crate::spotify::extract_spotify_track_id(txt) {
                let api2 = api.clone();
                let chat_id2 = message.chat.id;
                let msg_id2 = message.message_id;
                let db2 = database.clone();
                crate::app::spawn_user_task(async move {
                    if let Err(e) = crate::spotify::handle_spotify_url(
                        &api2, chat_id2, msg_id2, uid, trace_id, &track_id, &db2,
                    )
                    .await
                    {
                        crate::stats::record_error_global("spotify", e).await;
                    }
                });
                return Ok(true);
            }

            if let Some(sc_url) = crate::soundcloud::extract_soundcloud_url(txt) {
                let api2 = api.clone();
                let chat_id2 = message.chat.id;
                let msg_id2 = message.message_id;
                let db2 = database.clone();
                crate::app::spawn_user_task(async move {
                    if let Err(e) = crate::soundcloud::handle_soundcloud_url(
                        &api2, chat_id2, msg_id2, uid, trace_id, &sc_url, &db2,
                    )
                    .await
                    {
                        crate::stats::record_error_global("soundcloud", e).await;
                    }
                });
                return Ok(true);
            }

            let yt_urls = extract_youtube_urls(txt);
            if let Some(target_url) = yt_urls.into_iter().next() {
                let api2 = api.clone();
                let chat_id2 = message.chat.id;
                let msg_id2 = message.message_id;
                let pool2 = cookie_pool.clone();
                let db2 = database.clone();
                let rl_tx2 = rate_limit_tx.clone();
                crate::app::spawn_user_task(async move {
                    if let Err(e) = handle_youtube_url(
                        &api2,
                        chat_id2,
                        msg_id2,
                        Some(uid),
                        trace_id,
                        &target_url,
                        pool2,
                        &db2,
                        &rl_tx2,
                    )
                    .await
                    {
                        crate::stats::record_error_global("youtube", e).await;
                    }
                });
                return Ok(true);
            }

            let platform = crate::surge_dl::detect_social_platform(txt);
            if platform == Some("youtube") {
                // Handled above
            } else if platform == Some("spotify") {
                let api2 = api.clone();
                let chat_id2 = message.chat.id;
                crate::app::spawn_user_task(async move {
                    let _ =
                        crate::bot::send_text(&api2, chat_id2, &t("spotify.only_single_tracks"))
                            .await;
                });
                return Ok(true);
            } else if platform == Some("soundcloud") {
                let api2 = api.clone();
                let chat_id2 = message.chat.id;
                crate::app::spawn_user_task(async move {
                    let _ =
                        crate::bot::send_text(&api2, chat_id2, &t("soundcloud.only_single_tracks"))
                            .await;
                });
                return Ok(true);
            } else if let Some(p) = platform {
                let platform_name = t(&format!("platforms.{p}"));
                let text = tf(
                    "surge.unsupported_platform",
                    &[("platform", &platform_name)],
                );
                let api2 = api.clone();
                let chat_id2 = message.chat.id;
                crate::app::spawn_user_task(async move {
                    let _ = crate::bot::send_text(&api2, chat_id2, &text).await;
                    let _ = crate::bot::send_tools_menu(&api2, chat_id2).await;
                });
                return Ok(true);
            }

            let api2 = api.clone();
            let msg2 = message.clone();
            let fm2 = flow_manager.clone();
            let db2 = database.clone();
            crate::app::spawn_user_task(async move {
                handle_surge_text(&api2, &msg2, uid, &fm2, &db2).await;
            });
        }
        return Ok(true);
    }

    if matches!(
        flow_manager.get(uid),
        FlowState::AwaitingSurgeRenameInput { .. }
    ) {
        if message.text.is_some() {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "surge_dl_rename_route_dispatched",
                &format!("user_id={uid} chat_id={}", message.chat.id),
            );
            handle_surge_rename_text(api, &message, uid, flow_manager).await;
        }
        return Ok(true);
    }

    if let FlowState::AwaitingCompressPassword { config } = flow_manager.get(uid) {
        if message.text.is_some() {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "filecompress_password_dispatched",
                &format!("user_id={uid} chat_id={}", message.chat.id),
            );
            crate::filecompress::handle_fc_password_text(api, &message, uid, flow_manager, config)
                .await;
            return Ok(true);
        }
        // Non-text input during password step: prompt text required
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "filecompress_password_non_text",
            &format!("user_id={uid} chat_id={}", message.chat.id),
        );
        crate::filecompress::send_password_need_text(api, message.chat.id).await;
        return Ok(true);
    }

    if matches!(
        flow_manager.get(uid),
        FlowState::AwaitingCompressFiles { .. } | FlowState::AwaitingCompressOptions { .. }
    ) {
        if let Some(text) = &message.text {
            let trimmed = text.trim();
            if trimmed == t("fc.done_upload_button")
                || trimmed == "اتمام اپلود"
                || trimmed == "اتمام آپلود"
                || trimmed.contains("اتمام")
            {
                let trace_id = next_trace_id();
                log_trace(
                    trace_id,
                    "filecompress_done_dispatched",
                    &format!("user_id={uid} chat_id={}", message.chat.id),
                );
                crate::filecompress::handle_fc_done_text(
                    api,
                    &message,
                    uid,
                    flow_manager,
                    database,
                )
                .await;
                return Ok(true);
            }
            if trimmed == t("fc.cancel_button")
                || trimmed.contains(&t("emoji.cancel_button"))
                || trimmed == "انصراف"
                || trimmed == "لغو"
            {
                let config = match flow_manager.get(uid) {
                    FlowState::AwaitingCompressFiles { config, .. } => *config,
                    FlowState::AwaitingCompressOptions { config } => config,
                    _ => crate::filecompress::CompressConfig::default(),
                };
                flow_manager.set(
                    uid,
                    FlowState::AwaitingCompressOptions {
                        config: config.clone(),
                    },
                );
                let remove_params = frankenstein::methods::SendMessageParams::builder()
                    .chat_id(message.chat.id)
                    .text("\u{200B}")
                    .reply_markup(frankenstein::types::ReplyMarkup::ReplyKeyboardRemove(
                        frankenstein::types::ReplyKeyboardRemove::builder()
                            .remove_keyboard(true)
                            .build(),
                    ))
                    .build();
                if let Ok(res) = api.send_message(&remove_params).await {
                    let _ = api
                        .delete_message(
                            &frankenstein::methods::DeleteMessageParams::builder()
                                .chat_id(message.chat.id)
                                .message_id(res.result.message_id)
                                .build(),
                        )
                        .await;
                }
                crate::filecompress::send_options_menu(api, message.chat.id, &config).await;
                return Ok(true);
            }
        }

        if message.document.is_some()
            || message.video.is_some()
            || message.audio.is_some()
            || message.photo.is_some()
            || message.voice.is_some()
            || message.video_note.is_some()
            || message.animation.is_some()
        {
            let trace_id = next_trace_id();
            log_trace(
                trace_id,
                "filecompress_file_dispatched",
                &format!("user_id={uid} chat_id={}", message.chat.id),
            );
            crate::filecompress::handle_fc_file(api, &message, uid, flow_manager, database).await;
            return Ok(true);
        }
    }

    if matches!(flow_manager.get(uid), FlowState::AwaitingStudioTrimVideo) {
        if message.video.is_some() || message.document.is_some() {
            let trace_id = next_trace_id();
            log_ev!("studio_trim", trace_id, "video_dispatched", "user_id" => uid);
            let api2 = api.clone();
            let msg2 = message.clone();
            let mut fm = flow_manager.clone();
            flow_manager.clear(uid);
            crate::app::spawn_user_task(async move {
                crate::studio::trim::handle_video_upload(&api2, &msg2, uid, &mut fm).await;
            });
            return Ok(true);
        }
    }

    if matches!(
        flow_manager.get(uid),
        FlowState::AwaitingStudioCompressVideo
    ) {
        if message.video.is_some() || message.document.is_some() {
            let trace_id = next_trace_id();
            log_ev!("studio_compress", trace_id, "video_dispatched", "user_id" => uid);
            let api2 = api.clone();
            let msg2 = message.clone();
            let fm = flow_manager.clone();
            flow_manager.clear(uid);
            crate::app::spawn_user_task(async move {
                crate::studio::compress::handle_video_upload(&api2, msg2, uid, trace_id, &fm).await;
            });
            return Ok(true);
        }
    }

    if matches!(flow_manager.get(uid), FlowState::AwaitingStudioExtractVideo) {
        if message.video.is_some() || message.document.is_some() {
            let trace_id = next_trace_id();
            log_ev!("studio_extract", trace_id, "video_dispatched", "user_id" => uid);
            let api2 = api.clone();
            let msg2 = message.clone();
            let fm = flow_manager.clone();
            flow_manager.clear(uid);
            crate::app::spawn_user_task(async move {
                crate::studio::extract::handle_video_upload(&api2, msg2, uid, trace_id, &fm).await;
            });
            return Ok(true);
        }
    }

    if let FlowState::AwaitingStudioBurnInput { session } = flow_manager.get(uid) {
        if message.video.is_some() || message.document.is_some() {
            let trace_id = next_trace_id();
            log_ev!("studio_burn", trace_id, "input_dispatched", "user_id" => uid);
            let api2 = api.clone();
            let msg2 = message.clone();
            let mut fm = flow_manager.clone();
            let db2 = database.clone();
            crate::app::spawn_user_task(async move {
                crate::studio::burn::handle_input_message(
                    &api2, &msg2, uid, session, &mut fm, &db2,
                )
                .await;
            });
            return Ok(true);
        }
    }

    if let FlowState::AwaitingStudioTrimRanges {
        file_id,
        filename,
        duration_secs,
    } = flow_manager.get(uid)
    {
        if message.text.is_some() {
            let trace_id = next_trace_id();
            log_ev!("studio_trim", trace_id, "ranges_dispatched", "user_id" => uid);
            let api2 = api.clone();
            let msg2 = message.clone();
            let fm = flow_manager.clone();
            let db2 = database.clone();
            flow_manager.clear(uid);
            crate::app::spawn_user_task(async move {
                crate::studio::trim::handle_ranges_input(
                    &api2,
                    &msg2,
                    uid,
                    &file_id,
                    &filename,
                    duration_secs,
                    &fm,
                    db2,
                )
                .await;
            });
            return Ok(true);
        }
    }

    Ok(true)
}
