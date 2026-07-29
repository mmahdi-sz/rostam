use std::net::{IpAddr, SocketAddr};

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, Message},
};

use crate::bot::{edit_to_tools, send_text};
use crate::emoji::panel::btn_icon;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t};
use crate::log::next_trace_id;

use super::format::{cidr_note, format_report, format_special, port_note};
use super::lists;
use super::sources::fetch_all;

pub const CB_TOOLS_IP_LOOKUP: &str = "tools:ip_lookup";
pub const CB_IP_LOOKUP_CANCEL: &str = "ip_lookup:cancel";

fn cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(
            &t("start.back"),
            CB_IP_LOOKUP_CANCEL,
            "back",
        )]])
        .build()
}

pub async fn enter_ip_lookup(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_actor_id!("ip_lookup", trace_id, user_id, "clicked" => CB_TOOLS_IP_LOOKUP);
    flow_manager.set(user_id, FlowState::AwaitingIpLookupInput);
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(t("ip_lookup.prompt"))
        .reply_markup(cancel_keyboard())
        .build();
    let r = api.edit_message_text(&params).await;
    log_ev!("ip_lookup", trace_id, "prompt_shown", "=>" => if r.is_ok() { "ok" } else { "fail" });
}

pub async fn handle_ip_lookup_cancel(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_ev!("ip_lookup", trace_id, "cancel", "user_id" => user_id);
    flow_manager.clear(user_id);
    let _ = edit_to_tools(api, chat_id, message_id).await;
}

pub async fn handle_ip_lookup_text(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    log_actor_id!("ip_lookup", trace_id, user_id, "clicked" => "send_ip_text");

    let Some(text) = message.text.as_deref() else {
        return;
    };
    let Some((ip, note)) = parse_ip_input(text) else {
        log_ev!("ip_lookup", trace_id, "invalid_ip", "input" => text, "=>" => "reject");
        let _ = send_text(api, chat_id, &t("ip_lookup.invalid_ip")).await;
        return;
    };

    flow_manager.clear(user_id);
    run_ip_lookup(api.clone(), chat_id, user_id, ip, trace_id, note).await;
}

pub async fn handle_ip_command(api: &Bot, chat_id: i64, user_id: i64, arg: &str) {
    let trace_id = next_trace_id();
    log_actor_id!("ip_lookup", trace_id, user_id, "clicked" => "/ip");

    let Some((ip, note)) = parse_ip_input(arg) else {
        log_ev!("ip_lookup", trace_id, "invalid_ip", "input" => arg, "=>" => "reject");
        let _ = send_text(api, chat_id, &t("ip_lookup.invalid_ip")).await;
        return;
    };

    run_ip_lookup(api.clone(), chat_id, user_id, ip, trace_id, note).await;
}

/// Whole-message auto-detection: a plain chat message that is *only* an IP
/// (no `/ip` command, no flow state) still gets analyzed — mirrors how
/// YouTube links are auto-detected.
pub fn detect_ip(text: &str) -> Option<(IpAddr, Option<String>)> {
    parse_ip_input(text)
}

pub async fn handle_ip_lookup_auto(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    ip: IpAddr,
    note: Option<String>,
) {
    let trace_id = next_trace_id();
    log_actor_id!("ip_lookup", trace_id, user_id, "clicked" => "auto_detect_text");
    run_ip_lookup(api.clone(), chat_id, user_id, ip, trace_id, note).await;
}

/// Accepts a bare IP (`93.118.106.115`), `ip:port` (`93.118.106.115:500`), or
/// `ip/prefix` CIDR (`93.118.106.118/24`) — the last two get a short note
/// about the port/mask prepended before the usual IP analysis.
fn parse_ip_input(text: &str) -> Option<(IpAddr, Option<String>)> {
    let text = normalize_digits(text.trim());

    if let Ok(ip) = text.parse::<IpAddr>() {
        return Some((ip, None));
    }
    if let Ok(sock) = text.parse::<SocketAddr>() {
        return Some((sock.ip(), Some(port_note(sock.port()))));
    }
    let (ip_part, prefix_part) = text.split_once('/')?;
    let ip: IpAddr = ip_part.trim().parse().ok()?;
    let prefix: u8 = prefix_part.trim().parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    if prefix > max {
        return None;
    }
    Some((ip, Some(cidr_note(prefix, ip.is_ipv4()))))
}

/// Persian (۰-۹) and Arabic-Indic (٠-٩) digits → ASCII, so `std::net::IpAddr`'s
/// parser (which only understands ASCII) can still recognize the address.
fn normalize_digits(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '۰'..='۹' => (b'0' + (c as u32 - '۰' as u32) as u8) as char,
            '٠'..='٩' => (b'0' + (c as u32 - '٠' as u32) as u8) as char,
            other => other,
        })
        .collect()
}

// IANA special-purpose ranges (RFC 1918/3927/5737/etc) never carry real
// geolocation/ownership data — public lookup sources return garbage (random
// country, no org) instead of failing outright, so these are special-cased
// before hitting the network.
fn special_range(ip: &IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            if v4.is_loopback() {
                Some("ip_lookup.special.loopback")
            } else if v4.is_unspecified() {
                Some("ip_lookup.special.unspecified")
            } else if v4.is_broadcast() {
                Some("ip_lookup.special.broadcast")
            } else if v4.is_link_local() {
                Some("ip_lookup.special.link_local")
            } else if v4.is_documentation() {
                Some("ip_lookup.special.documentation")
            } else if v4.is_private() {
                Some("ip_lookup.special.private")
            } else if v4.is_multicast() {
                Some("ip_lookup.special.multicast")
            } else if o[0] == 100 && (64..=127).contains(&o[1]) {
                Some("ip_lookup.special.cgnat")
            }
            // RFC 6598
            else if o[0] == 192 && o[1] == 0 && o[2] == 0 {
                Some("ip_lookup.special.ietf_protocol")
            }
            // RFC 6890
            else if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
                Some("ip_lookup.special.benchmarking")
            }
            // RFC 2544
            else if o[0] >= 240 {
                Some("ip_lookup.special.reserved")
            }
            // Class E
            else {
                None
            }
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return special_range(&IpAddr::V4(mapped));
            }
            let s = v6.segments();
            if v6.is_loopback() {
                Some("ip_lookup.special.loopback")
            } else if v6.is_unspecified() {
                Some("ip_lookup.special.unspecified")
            } else if v6.is_unique_local() {
                Some("ip_lookup.special.private")
            } else if v6.is_unicast_link_local() {
                Some("ip_lookup.special.link_local")
            } else if v6.is_multicast() {
                Some("ip_lookup.special.multicast")
            } else if s[0] == 0x2001 && s[1] == 0x0db8 {
                Some("ip_lookup.special.documentation_v6")
            }
            // RFC 3849
            else if s[0] == 0x2001 && s[1] == 0 {
                Some("ip_lookup.special.teredo")
            }
            // RFC 4380
            else if s[0] == 0x2002 {
                Some("ip_lookup.special.six_to_four")
            }
            // RFC 3056
            else {
                None
            }
        }
    }
}

async fn run_ip_lookup(
    api: Bot,
    chat_id: i64,
    user_id: i64,
    ip: IpAddr,
    trace_id: u64,
    note: Option<String>,
) {
    let (mut card_md, status_msg_id) = if let Some(key) = special_range(&ip) {
        log_ev!("ip_lookup", trace_id, "special_range", "kind" => key);
        (format_special(&ip.to_string(), key), None)
    } else {
        let status = SendMessageParams::builder()
            .chat_id(chat_id)
            .text(t("ip_lookup.processing"))
            .build();
        let status_msg_id = api
            .send_message(&status)
            .await
            .ok()
            .map(|r| r.result.message_id);

        log_ev!("ip_lookup", trace_id, "fetch_start", "ip" => ip);
        let ip_str = ip.to_string();
        let (report, matches) = tokio::join!(fetch_all(&ip_str, trace_id), lists::classify(ip));
        log_ev!("ip_lookup", trace_id, "fetch_done");

        (format_report(&report, &matches), status_msg_id)
    };
    if let Some(n) = note {
        card_md = format!("{n}\n\n{card_md}");
    }
    let card = apply_premium_to_md(&card_md);

    let sent = if let Some(message_id) = status_msg_id {
        api.edit_message_text(
            &EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(message_id)
                .text(&card)
                .parse_mode(ParseMode::MarkdownV2)
                .build(),
        )
        .await
        .map(|_| ())
        .map_err(anyhow::Error::from)
    } else {
        crate::bot::send_text_md(&api, chat_id, &card).await
    };

    match sent {
        Ok(()) => {
            log_ev!("ip_lookup", trace_id, "result_sent", "=>" => "ok");
            crate::stats::record_event_user(user_id, "ip_lookup", "lookup", "ok", 0).await;
        }
        Err(e) => {
            log_ev!("ip_lookup", trace_id, "result_send_failed", "=>" => format!("fail err={e}"));
            crate::stats::record_error_global("ip_lookup", &format!("send failed: {e}")).await;
            crate::stats::record_event_user(user_id, "ip_lookup", "lookup", "fail", 0).await;
        }
    }
}

#[cfg(test)]
mod parse_tests {
    use super::parse_ip_input;

    #[test]
    fn bare_ip_no_note() {
        let (ip, note) = parse_ip_input("93.118.106.115").unwrap();
        assert_eq!(ip.to_string(), "93.118.106.115");
        assert!(note.is_none());
    }

    #[test]
    fn ip_with_port() {
        let (ip, note) = parse_ip_input("93.118.106.115:500").unwrap();
        assert_eq!(ip.to_string(), "93.118.106.115");
        assert!(note.unwrap().contains("500"));
    }

    #[test]
    fn ip_with_cidr() {
        let (ip, note) = parse_ip_input("93.118.106.118/24").unwrap();
        assert_eq!(ip.to_string(), "93.118.106.118");
        assert!(note.unwrap().contains("/24"));
    }

    #[test]
    fn persian_digits() {
        let (ip, _) = parse_ip_input("۱۲۷.۰.۰.۱").unwrap();
        assert_eq!(ip.to_string(), "127.0.0.1");
    }

    #[test]
    fn cidr_prefix_out_of_range_rejected() {
        assert!(parse_ip_input("93.118.106.118/33").is_none());
    }

    #[test]
    fn garbage_rejected() {
        assert!(parse_ip_input("not an ip").is_none());
    }

    #[test]
    fn ipv4_cidr_note_has_host_count() {
        let (_, note) = parse_ip_input("93.118.106.118/24").unwrap();
        assert!(note.unwrap().contains("پوشش می‌ده"));
    }

    #[test]
    fn ipv6_cidr_note_has_no_host_count() {
        let (_, note) = parse_ip_input("2001:db8::/64").unwrap();
        let note = note.unwrap();
        assert!(note.contains("/64"));
        assert!(!note.contains("پوشش می‌ده"));
    }
}
