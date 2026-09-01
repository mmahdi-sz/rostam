use crate::app::state::AppState;
use crate::bot::{send_lang_picker, send_start_menu};
use crate::config;
use crate::emoji::{FlowState, handler as emoji_handler};
use crate::i18n::{reload_i18n, t, tf};
use crate::ip_lookup::{detect_ip, handle_ip_command, handle_ip_lookup_auto};
use crate::log::next_trace_id;
use crate::stats;
use crate::surge_dl::handle_surge_text;
use crate::youtube::trace::log_trace;
use crate::youtube::{extract_youtube_urls, handle_youtube_url};

use super::flow;

pub(super) async fn handle_message(
    state: &mut AppState,
    message: frankenstein::types::Message,
) -> crate::error::Result<()> {
    if message.from.as_ref().map_or(false, |u| u.is_bot) || message.pinned_message.is_some() {
        return Ok(());
    }
    let AppState {
        api,
        cookie_pool,
        database,
        flow_manager,
        rate_limit_tx,
        ..
    } = state;
    let user_id = message.from.as_ref().map(|u| u.id as i64);
    let msg_text = message
        .text
        .as_deref()
        .or(message.caption.as_deref())
        .unwrap_or("");
    if let Some(u) = message.from.as_ref() {
        let trace_id = crate::log::next_trace_id();
        log_actor!("dispatch", trace_id, u, "msg" => msg_text.chars().take(40).collect::<String>());
    }

    // Register user in stats. (Referral attribution already ran before the
    // language gate — a brand-new user never reaches this point.)
    if let Some(uid) = user_id {
        let username = message.from.as_ref().and_then(|u| u.username.as_deref());
        stats::record_user_global(uid, username).await;
    }

    // Step 1: addemoji link detection
    if let Some(uid) = user_id {
        if let Some(text) = message.text.as_deref().or(message.caption.as_deref()) {
            if !text.trim_start().starts_with('/') {
                if let Some(pack_name) = emoji_handler::extract_addemoji_pack_name(text) {
                    emoji_handler::handle_addemoji_link(
                        api,
                        &message,
                        uid,
                        &pack_name,
                        flow_manager,
                        database,
                    )
                    .await;
                    return Ok(());
                }
            }
        }
    }

    // Step 2: /start always clears flow (+ deep-link payload: redeem<CODE>)
    if let (Some(uid), Some(text)) = (
        user_id,
        message.text.as_deref().or(message.caption.as_deref()),
    ) {
        if let Some(rest) = text.strip_prefix("/start") {
            flow_manager.clear(uid);
            let payload = rest.trim();
            if let Some(code) = payload.strip_prefix("redeem") {
                let first_name = message
                    .from
                    .as_ref()
                    .map(|u| u.first_name.as_str())
                    .unwrap_or("");
                let username = message.from.as_ref().and_then(|u| u.username.as_deref());
                crate::redeem::handle::handle_redeem(
                    api,
                    message.chat.id,
                    uid,
                    first_name,
                    username,
                    code,
                    database,
                )
                .await;
            } else {
                // Referral payload recorded earlier, before the language gate.
                send_start_menu(api, message.chat.id).await?;
            }
            return Ok(());
        }
    }

    // Step 3: "Cancel operation" reply keyboard when Idle
    if let (Some(uid), Some(text)) = (
        user_id,
        message.text.as_deref().or(message.caption.as_deref()),
    ) {
        if text.contains(&crate::i18n::t("emoji.cancel_button"))
            && matches!(flow_manager.get(uid), FlowState::Idle)
        {
            send_start_menu(api, message.chat.id).await?;
            return Ok(());
        }
    }

    // Step 4: active flow dispatch
    if let Some(uid) = user_id {
        if flow::handle_flow_message(
            api,
            cookie_pool,
            database,
            flow_manager,
            rate_limit_tx,
            &message,
            uid,
        )
        .await?
        {
            return Ok(());
        }
    }

    // Step 5: command dispatch
    if let Some(text) = message.text.as_deref().or(message.caption.as_deref()) {
        let cmd = text.split('@').next().unwrap_or(text);
        eprintln!("[dispatch event=cmd] user_id={user_id:?} cmd={cmd:?}");
        if cmd == "/ref" || cmd == "/referral" {
            if let Some(uid) = user_id {
                crate::rank::panel::send_referral(api, message.chat.id, uid, database).await;
            }
            return Ok(());
        }
        if cmd == "/rank" {
            eprintln!(
                "[dispatch event=rank_menu] user_id={user_id:?} chat_id={}",
                message.chat.id
            );
            crate::rank::menu::send_rank_menu(api, message.chat.id).await;
            return Ok(());
        }
        if cmd == "/panel" {
            if let Some(uid) = user_id {
                crate::rank::panel::send_user_panel(api, message.chat.id, uid, database).await;
            }
            return Ok(());
        }
        if cmd == "/language" {
            send_lang_picker(api, message.chat.id).await?;
            return Ok(());
        }
        if let Some(rest) = text.strip_prefix("/ip ") {
            if let Some(uid) = user_id {
                handle_ip_command(api, message.chat.id, uid, rest.trim()).await;
            }
            return Ok(());
        }
        // Custom emoji pack panel (admin only, hidden from every user-facing menu).
        if cmd == "/emoji" {
            let trace = next_trace_id();
            let is_admin = config::admin_user_id()
                .map(|id| Some(id) == user_id)
                .unwrap_or(false);
            log_trace(
                trace,
                "cmd_emoji",
                &format!("user_id={user_id:?} is_admin={is_admin}"),
            );
            if is_admin {
                emoji_handler::handle_emoji_command(api, &message, flow_manager, database).await;
            }
            return Ok(());
        }
        // Generate gift code (admin only): /re 30d es 1u or /re
        if cmd == "/re" || text.starts_with("/re ") {
            let is_admin = config::admin_user_id()
                .map(|id| Some(id) == user_id)
                .unwrap_or(false);
            if is_admin {
                if let Some(uid) = user_id {
                    let rest = text.split_once(' ').map(|(_, r)| r.trim()).unwrap_or("");
                    crate::redeem::handle::handle_generate(
                        api,
                        message.chat.id,
                        uid,
                        rest,
                        database,
                    )
                    .await;
                }
            }
            return Ok(());
        }
        match text {
            "/i18n_reload" => {
                let is_admin = config::admin_user_id()
                    .map(|id| Some(id) == user_id)
                    .unwrap_or(false);
                if is_admin {
                    reload_i18n();
                    crate::bot::send_text(api, message.chat.id, "✅ config/i18n.json reloaded.")
                        .await?;
                }
            }
            "/start" => send_start_menu(api, message.chat.id).await?,

            _ => {
                if let (Some(uid), Some((ip, note))) = (user_id, detect_ip(text)) {
                    handle_ip_lookup_auto(api, message.chat.id, uid, ip, note).await;
                    return Ok(());
                }

                if let Some(uid) = user_id {
                    if crate::musicset::try_route_set(api, message.chat.id, uid, database, text) {
                        return Ok(());
                    }
                    if let Some(track_id) = crate::spotify::extract_spotify_track_id(text) {
                        let trace_id = next_trace_id();
                        log_trace(
                            trace_id,
                            "route_spotify_url",
                            &format!(
                                "user_id={uid} chat_id={} track_id={track_id}",
                                message.chat.id
                            ),
                        );
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
                        return Ok(());
                    }

                    if let Some(sc_url) = crate::soundcloud::extract_soundcloud_url(text) {
                        let trace_id = next_trace_id();
                        log_trace(
                            trace_id,
                            "route_soundcloud_url",
                            &format!("user_id={uid} chat_id={} url={sc_url}", message.chat.id),
                        );
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
                        return Ok(());
                    }

                    let yt_urls = extract_youtube_urls(text);
                    if let Some(target_url) = yt_urls.into_iter().next() {
                        let trace_id = next_trace_id();
                        log_trace(
                            trace_id,
                            "route_youtube_url",
                            &format!("user_id={uid} chat_id={} url={target_url}", message.chat.id),
                        );
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
                        return Ok(());
                    }

                    if let Some(platform) = crate::surge_dl::detect_social_platform(text) {
                        if platform == "youtube" {
                            // Handled above
                        } else if platform == "spotify" {
                            let api2 = api.clone();
                            let chat_id2 = message.chat.id;
                            crate::app::spawn_user_task(async move {
                                let _ = crate::bot::send_text(
                                    &api2,
                                    chat_id2,
                                    &t("spotify.only_single_tracks"),
                                )
                                .await;
                            });
                            return Ok(());
                        } else if platform == "soundcloud" {
                            let api2 = api.clone();
                            let chat_id2 = message.chat.id;
                            crate::app::spawn_user_task(async move {
                                let _ = crate::bot::send_text(
                                    &api2,
                                    chat_id2,
                                    &t("soundcloud.only_single_tracks"),
                                )
                                .await;
                            });
                            return Ok(());
                        } else {
                            let trace_id = next_trace_id();
                            log_trace(
                                trace_id,
                                "route_unsupported_social_platform",
                                &format!(
                                    "user_id={uid} chat_id={} platform={platform} url={text}",
                                    message.chat.id
                                ),
                            );
                            let platform_name = t(&format!("platforms.{platform}"));
                            let msg_text = tf(
                                "surge.unsupported_platform",
                                &[("platform", &platform_name)],
                            );
                            let api2 = api.clone();
                            let chat_id2 = message.chat.id;
                            crate::app::spawn_user_task(async move {
                                let _ = crate::bot::send_text(&api2, chat_id2, &msg_text).await;
                                let _ = crate::bot::send_tools_menu(&api2, chat_id2).await;
                            });
                            return Ok(());
                        }
                    }

                    if crate::surge_dl::is_direct_link(text) {
                        let trace_id = next_trace_id();
                        log_trace(
                            trace_id,
                            "route_surge_dl_url",
                            &format!("user_id={uid} chat_id={} url={text}", message.chat.id),
                        );
                        let api2 = api.clone();
                        let msg2 = message.clone();
                        let fm2 = flow_manager.clone();
                        let db2 = database.clone();
                        crate::app::spawn_user_task(async move {
                            handle_surge_text(&api2, &msg2, uid, &fm2, &db2).await;
                        });
                    }
                }
            }
        }
    }
    Ok(())
}
