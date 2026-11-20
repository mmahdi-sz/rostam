use crate::app::state::AppState;
use crate::bot::{send_lang_picker, send_start_menu};
use crate::config;
use crate::emoji::{FlowManager, FlowState, handler as emoji_handler};
use crate::i18n::{reload_i18n, t, tf};
use crate::ip_lookup::{detect_ip, handle_ip_command, handle_ip_lookup_auto};
use crate::log::next_trace_id;
use crate::stats;
use crate::surge_dl::handle_surge_text;
use crate::youtube::trace::log_trace;
use crate::youtube::{extract_youtube_urls, handle_youtube_url};

use super::flow;

pub(crate) async fn handle_message(
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
    let standalone_link = standalone_web_link(&message);
    let dispatch_trace = next_trace_id();
    if let Some(u) = message.from.as_ref() {
        let summary = if standalone_link.is_some() {
            "[link]".to_owned()
        } else {
            msg_text.chars().take(40).collect()
        };
        log_actor!("dispatch", dispatch_trace, u, "msg" => summary);
    }

    // Register user in stats. (Referral attribution already ran before the
    // language gate — a brand-new user never reaches this point.)
    if let Some(uid) = user_id {
        let username = message.from.as_ref().and_then(|u| u.username.as_deref());
        stats::record_user_global(uid, username).await;
    }

    let owns_link = user_id.is_some_and(|uid| flow_manager.get(uid).expects_link());
    let route_link = standalone_link
        .as_ref()
        .filter(|_| user_id.is_some() && !owns_link);
    if let (Some(uid), Some(_)) = (user_id, route_link) {
        log_ev!("dispatch", dispatch_trace, "link_exit", "=>" => "route");
        exit_waiting_flow(flow_manager, uid);
    }

    // Step 1: addemoji link detection
    if let Some(uid) = user_id.filter(|_| !(owns_link && standalone_link.is_some())) {
        if let Some(text) = message.text.as_deref().or(message.caption.as_deref()) {
            if !text.trim_start().starts_with('/') {
                if let Some(pack_name) = emoji_handler::extract_addemoji_pack_name(text) {
                    if route_link.is_some() {
                        log_ev!("dispatch", dispatch_trace, "link_route", "handler" => "addemoji");
                    }
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
            exit_waiting_flow(flow_manager, uid);
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

    if let Some(uid) = user_id {
        let current_flow = flow_manager.get(uid);
        let youtube_url = if owns_link
            && standalone_link.is_some()
            && !matches!(current_flow, FlowState::AwaitingSurgeUrlInput)
        {
            None
        } else if let Some(link) = route_link {
            extract_youtube_urls(link).into_iter().next()
        } else {
            youtube_url_for_flow(&message, &current_flow)
        };
        if let Some(target_url) = youtube_url {
            if matches!(current_flow, FlowState::AwaitingSurgeUrlInput) {
                flow_manager.clear(uid);
            }
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
                    log_ev!("yt", trace_id, "handle_url_failed", "err" => format!("{e}"));
                }
            });
            return Ok(());
        }
    }

    // Step 4: active flow dispatch
    if let Some(uid) = user_id.filter(|_| route_link.is_none()) {
        if flow::handle_flow_message(api, database, flow_manager, &message, uid).await? {
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
                    let candidate_links = crate::common::link::extract_message_links(&message);
                    let mut candidates = match route_link {
                        Some(link) => vec![link.clone()],
                        None => candidate_links,
                    };
                    let raw_trimmed = text.trim();
                    if route_link.is_none()
                        && !raw_trimmed.is_empty()
                        && !candidates.iter().any(|c| c == raw_trimmed)
                    {
                        candidates.push(raw_trimmed.to_string());
                    }

                    for candidate in candidates {
                        let resolved_candidate;
                        let cand = if candidate.contains(concat!("spotify", ".link")) {
                            resolved_candidate =
                                crate::spotify::extract::resolve_spotify_shortlink(&candidate)
                                    .await
                                    .unwrap_or_else(|| candidate.clone());
                            &resolved_candidate
                        } else {
                            &candidate
                        };

                        if crate::spotify::extract::is_spotify_podcast(cand) {
                            let _ = crate::bot::send_text(
                                api,
                                message.chat.id,
                                &crate::i18n::t("spotify.podcast_unsupported"),
                            )
                            .await;
                            return Ok(());
                        }

                        if crate::musicset::try_route_set(api, message.chat.id, uid, database, cand)
                        {
                            return Ok(());
                        }
                        if let Some(track_id) = crate::spotify::extract_spotify_track_id(cand) {
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

                        if let Some(sc_url) = crate::soundcloud::extract_soundcloud_url(cand) {
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

                        if let Some(platform) = crate::surge_dl::detect_social_platform(cand) {
                            if platform == "youtube" {
                                continue;
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
                                        "user_id={uid} chat_id={} platform={platform} url={cand}",
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

                        if crate::surge_dl::is_direct_link(cand) {
                            let trace_id = next_trace_id();
                            log_trace(
                                trace_id,
                                "route_surge_dl_url",
                                &format!("user_id={uid} chat_id={} url={cand}", message.chat.id),
                            );
                            let api2 = api.clone();
                            let mut msg2 = message.clone();
                            if let Some(link) = route_link {
                                msg2.text = Some(link.clone());
                                msg2.entities = None;
                            }
                            let fm2 = flow_manager.clone();
                            let db2 = database.clone();
                            crate::app::spawn_user_task(async move {
                                handle_surge_text(&api2, &msg2, uid, &fm2, &db2).await;
                            });
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
    if route_link.is_some() {
        log_ev!("dispatch", dispatch_trace, "link_unsupported", "=>" => "reply");
        if let Err(error) =
            crate::bot::send_text_md(api, message.chat.id, &t("unsupported_link")).await
        {
            stats::record_error_global("dispatch", &error).await;
            return Err(error);
        }
    }
    Ok(())
}

fn exit_waiting_flow(flow_manager: &mut FlowManager, uid: i64) {
    flow_manager.exit(uid);
}

fn standalone_web_link(message: &frankenstein::types::Message) -> Option<String> {
    use frankenstein::types::MessageEntityType;

    let text = message.text.as_deref()?;
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    let url = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        let start = text[..text.len() - text.trim_start().len()]
            .encode_utf16()
            .count();
        let len = trimmed.encode_utf16().count();
        message.entities.as_ref()?.iter().find(|entity| {
            entity.type_field == MessageEntityType::Url
                && entity.offset as usize == start
                && entity.length as usize == len
        })?;
        format!("https://{trimmed}")
    };
    let parsed = reqwest::Url::parse(&url).ok()?;
    (matches!(parsed.scheme(), "http" | "https") && parsed.has_host()).then_some(url)
}

fn youtube_url_for_flow(
    message: &frankenstein::types::Message,
    flow: &FlowState,
) -> Option<String> {
    if !matches!(
        flow,
        FlowState::Idle | FlowState::AwaitingSurgeConfirm { .. } | FlowState::AwaitingSurgeUrlInput
    ) {
        return None;
    }
    crate::common::link::extract_message_links(message)
        .iter()
        .map(String::as_str)
        .chain(message.text.as_deref().or(message.caption.as_deref()))
        .find_map(|candidate| extract_youtube_urls(candidate).into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_standalone_text_url_exits_the_flow() {
        for (input, accepted) in [
            ("  https://example.com/file  ", true),
            ("watch https://example.com/file", false),
            ("https://a.example https://b.example", false),
            ("ftp://example.com/file", false),
        ] {
            let message = serde_json::from_value(serde_json::json!({
                "message_id": 1,
                "date": 0,
                "chat": {"id": 1, "type": "private"},
                "text": input,
            }))
            .unwrap();
            assert_eq!(standalone_web_link(&message).is_some(), accepted);
        }
    }

    #[test]
    fn youtube_links_are_routed_while_surge_confirmation_is_pending() {
        let pending = FlowState::AwaitingSurgeConfirm {
            url: "https://example.com/file.zip".into(),
            filename: "file.zip".into(),
        };
        for (input, expected) in [
            (
                "http://youtube.co/@DavidHoffmanFilmmaker",
                Some("http://www.youtube.com/@DavidHoffmanFilmmaker"),
            ),
            (
                "https://www.youtube.com/channel/UCddem5RlB3bQe99wyY49g0g",
                Some("https://www.youtube.com/channel/UCddem5RlB3bQe99wyY49g0g"),
            ),
            (
                "https://youtu.be/i7ydo7H_1TA",
                Some("https://youtu.be/i7ydo7H_1TA"),
            ),
            (
                "music.youtube.com/watch?v=i7ydo7H_1TA",
                Some("https://music.youtube.com/watch?v=i7ydo7H_1TA"),
            ),
            (
                "https://www.youtube-nocookie.com/embed/i7ydo7H_1TA",
                Some("https://www.youtube.com/embed/i7ydo7H_1TA"),
            ),
            ("https://youtube.co.example.com/file.zip", None),
            ("https://youtube.com@example.com/file.zip", None),
        ] {
            let message = serde_json::from_value(serde_json::json!({
                "message_id": 1,
                "date": 0,
                "chat": {"id": 1, "type": "private"},
                "text": input,
            }))
            .unwrap();
            assert_eq!(
                youtube_url_for_flow(&message, &pending).as_deref(),
                expected
            );
            assert_eq!(
                youtube_url_for_flow(&message, &FlowState::Idle).as_deref(),
                expected
            );
        }
    }
}
