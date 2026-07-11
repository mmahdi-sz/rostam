use std::net::IpAddr;

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

use super::format::format_report;
use super::lists;
use super::sources::fetch_all;

pub const CB_TOOLS_IP_LOOKUP: &str = "tools:ip_lookup";
pub const CB_IP_LOOKUP_CANCEL: &str = "ip_lookup:cancel";

fn cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(&t("start.back"), CB_IP_LOOKUP_CANCEL, "back")]])
        .build()
}

pub async fn enter_ip_lookup(
    api: &Bot, chat_id: i64, message_id: i32, user_id: i64, flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_actor_id!("ip_lookup", trace_id, user_id, "clicked" => CB_TOOLS_IP_LOOKUP);
    flow_manager.set(user_id, FlowState::AwaitingIpLookupInput);
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id).message_id(message_id)
        .text(t("ip_lookup.prompt"))
        .reply_markup(cancel_keyboard())
        .build();
    let r = api.edit_message_text(&params).await;
    log_ev!("ip_lookup", trace_id, "prompt_shown", "=>" => if r.is_ok() { "ok" } else { "fail" });
}

pub async fn handle_ip_lookup_cancel(
    api: &Bot, chat_id: i64, message_id: i32, user_id: i64, flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_ev!("ip_lookup", trace_id, "cancel", "user_id" => user_id);
    flow_manager.clear(user_id);
    let _ = edit_to_tools(api, chat_id, message_id).await;
}

pub async fn handle_ip_lookup_text(
    api: &Bot, message: &Message, user_id: i64, flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    log_actor_id!("ip_lookup", trace_id, user_id, "clicked" => "send_ip_text");

    let Some(text) = message.text.as_deref() else { return };
    let Some(ip) = parse_ip(text) else {
        log_ev!("ip_lookup", trace_id, "invalid_ip", "input" => text, "=>" => "reject");
        let _ = send_text(api, chat_id, &t("ip_lookup.invalid_ip")).await;
        return;
    };

    flow_manager.clear(user_id);
    run_ip_lookup(api.clone(), chat_id, user_id, ip, trace_id).await;
}

pub async fn handle_ip_command(api: &Bot, chat_id: i64, user_id: i64, arg: &str) {
    let trace_id = next_trace_id();
    log_actor_id!("ip_lookup", trace_id, user_id, "clicked" => "/ip");

    let Some(ip) = parse_ip(arg) else {
        log_ev!("ip_lookup", trace_id, "invalid_ip", "input" => arg, "=>" => "reject");
        let _ = send_text(api, chat_id, &t("ip_lookup.invalid_ip")).await;
        return;
    };

    run_ip_lookup(api.clone(), chat_id, user_id, ip, trace_id).await;
}

fn parse_ip(text: &str) -> Option<IpAddr> {
    text.trim().parse().ok()
}

async fn run_ip_lookup(api: Bot, chat_id: i64, user_id: i64, ip: IpAddr, trace_id: u64) {
    let status = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(t("ip_lookup.processing"))
        .build();
    let status_msg_id = api.send_message(&status).await.ok().map(|r| r.result.message_id);

    log_ev!("ip_lookup", trace_id, "fetch_start", "ip" => ip);
    let ip_str = ip.to_string();
    let (report, matches) = tokio::join!(fetch_all(&ip_str, trace_id), lists::classify(ip));
    log_ev!("ip_lookup", trace_id, "fetch_done");

    let card = apply_premium_to_md(&format_report(&report, &matches));

    let sent = if let Some(message_id) = status_msg_id {
        api.edit_message_text(
            &EditMessageTextParams::builder()
                .chat_id(chat_id).message_id(message_id)
                .text(&card)
                .parse_mode(ParseMode::MarkdownV2)
                .build(),
        ).await.map(|_| ()).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
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
