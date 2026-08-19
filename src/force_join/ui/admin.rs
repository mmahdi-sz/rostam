use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::SendMessageParams,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup},
};

use crate::config;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::panel::{btn_icon, btn_icon_plain};
use crate::force_join::conn::{already_count_key, conn, linked_count_key};
use crate::force_join::db::{
    add_lock, get_lock, is_enabled, list_locks, set_display_name, set_member_cap,
    set_reserve_link, set_time_limit,
};
use crate::force_join::jalali::{fmt_jalali_dt, now_epoch};
use crate::force_join::types::{
    CB_FJ_ADD_CANCEL, CB_FJ_ADD_NEW, CB_FJ_NOOP, CB_FJ_TOGGLE, CB_FJ_VIEW, FJ_DELETE_PREFIX,
    FJ_DELETE_YES_PREFIX, FJ_MANAGE_PREFIX, FJ_MEMBER_PREFIX, FJ_MODE_PREFIX, FJ_NAME_PREFIX,
    FJ_RESERVE_PREFIX, FJ_TIME_PREFIX, Lock,
};
use crate::force_join::ui::{edit_text_np, no_preview, send_text_np};
use crate::i18n::{t, tf};

pub fn menu_text() -> String {
    let username = config::bot_username();
    let at_username = if username.is_empty() {
        String::new()
    } else {
        format!("@{username}")
    };
    tf("force_join.info_text", &[("bot_username", &at_username)])
}

pub fn menu_keyboard(enabled: bool) -> InlineKeyboardMarkup {
    let status_text = if enabled {
        t("force_join.status_on")
    } else {
        t("force_join.status_off")
    };
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon(&status_text, CB_FJ_TOGGLE, "")],
            vec![btn_icon_plain("\u{200F}", CB_FJ_NOOP, "")],
            vec![btn_icon(&t("force_join.view_button"), CB_FJ_VIEW, "")],
            vec![btn_icon(
                &t("force_join.back_to_admin_button"),
                crate::bot::CB_ADMIN_PANEL,
                "back",
            )],
        ])
        .build()
}

/// Displays or refreshes "Force join" submenu in admin panel.
pub async fn open_menu(api: &Bot, chat_id: i64, message_id: i32) {
    let enabled = is_enabled().await;
    let _ = edit_text_np(
        api,
        chat_id,
        message_id,
        &menu_text(),
        Some(menu_keyboard(enabled)),
    )
    .await;
}

pub fn locks_list_view(locks: &[Lock]) -> (String, InlineKeyboardMarkup) {
    use crate::bot::CB_ADMIN_FORCE_JOIN;

    if locks.is_empty() {
        let kb = InlineKeyboardMarkup::builder()
            .inline_keyboard(vec![vec![btn_icon(
                &t("force_join.add_new_button"),
                CB_FJ_ADD_NEW,
                "",
            )]])
            .build();
        return (t("force_join.no_locks_text"), kb);
    }

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    for lock in locks {
        let manage_cb = format!("{FJ_MANAGE_PREFIX}{}", lock.id);
        rows.push(vec![
            btn_icon_plain(&lock.display_name(), CB_FJ_NOOP, ""),
            btn_icon(&t("force_join.manage_button"), &manage_cb, ""),
        ]);
    }
    rows.push(vec![btn_icon_plain("\u{200F}", CB_FJ_NOOP, "")]);
    rows.push(vec![btn_icon(
        &t("force_join.add_new_button"),
        CB_FJ_ADD_NEW,
        "",
    )]);
    rows.push(vec![btn_icon(
        &t("force_join.back_to_prev_button"),
        CB_ADMIN_FORCE_JOIN,
        "back",
    )]);

    (
        t("force_join.locks_list_title"),
        InlineKeyboardMarkup::builder()
            .inline_keyboard(rows)
            .build(),
    )
}

/// Renders "View locks" page (edits existing message).
pub async fn open_locks_list(api: &Bot, chat_id: i64, message_id: i32) {
    let (text, kb) = locks_list_view(&list_locks().await);
    edit_text_np(api, chat_id, message_id, &text, Some(kb)).await;
}

/// Same locks list as a new message — for immediate display after adding lock.
pub async fn send_locks_list(api: &Bot, chat_id: i64) {
    let (text, kb) = locks_list_view(&list_locks().await);
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .link_preview_options(no_preview())
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(kb))
        .build();
    let _ = api.send_message(&params).await;
}

/// Text and keyboard for "⚙️ Manage lock" panel. Returns None if lock not found.
pub async fn build_manage(
    lock_id: i64,
    database: &Option<PostgresDatabase>,
) -> Option<(String, InlineKeyboardMarkup)> {
    let lock = get_lock(lock_id).await?;

    let Ok(mut c) = conn().await else { return None };
    let already_joined: i64 = redis::cmd("GET")
        .arg(already_count_key(lock_id))
        .query_async(&mut c)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    let joined_via_link: i64 = redis::cmd("GET")
        .arg(linked_count_key(lock_id))
        .query_async(&mut c)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);

    let total_users = match database {
        Some(db) => {
            if let Ok(client) = db.get().await {
                crate::stats::get_user_stats(&client)
                    .await
                    .map(|s| s.total)
                    .unwrap_or(0)
            } else {
                0
            }
        }
        None => 0,
    };
    let not_joined_or_left = (total_users - already_joined - joined_via_link).max(0);
    let identifier_display = if lock.identifier.is_empty() {
        "—".to_string()
    } else {
        lock.identifier.clone()
    };

    let text = tf(
        "force_join.status_text",
        &[
            ("link", &lock.link),
            ("channel_username", &identifier_display),
            ("since_date", &fmt_jalali_dt(lock.created_at)),
            ("started_count", &total_users.to_string()),
            ("joined_via_link_count", &joined_via_link.to_string()),
            ("already_joined_count", &already_joined.to_string()),
            ("not_joined_count", &not_joined_or_left.to_string()),
            ("now_date", &fmt_jalali_dt(now_epoch())),
        ],
    );

    let none = t("force_join.limit_none");
    let mode_value = if lock.is_mandatory() {
        t("force_join.mode_mandatory")
    } else {
        t("force_join.mode_optional")
    };
    let time_value = if lock.expires_at == 0 {
        none.clone()
    } else {
        fmt_jalali_dt(lock.expires_at)
    };
    let member_value = if lock.member_cap == 0 {
        none.clone()
    } else {
        lock.member_cap.to_string()
    };

    // RTL layout: [value, label] → value on left, label on right.
    let name_cb = format!("{FJ_NAME_PREFIX}{lock_id}");
    let mode_cb = format!("{FJ_MODE_PREFIX}{lock_id}");
    let time_cb = format!("{FJ_TIME_PREFIX}{lock_id}");
    let member_cb = format!("{FJ_MEMBER_PREFIX}{lock_id}");
    let reserve_cb = format!("{FJ_RESERVE_PREFIX}{lock_id}");
    let delete_cb = format!("{FJ_DELETE_PREFIX}{lock_id}");

    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                btn_icon(&lock.display_name(), &name_cb, ""),
                btn_icon(&t("force_join.field_name"), &name_cb, ""),
            ],
            vec![
                btn_icon(&mode_value, &mode_cb, ""),
                btn_icon(&t("force_join.field_mode"), &mode_cb, ""),
            ],
            vec![
                btn_icon(&time_value, &time_cb, ""),
                btn_icon(&t("force_join.field_time"), &time_cb, ""),
            ],
            vec![
                btn_icon(&member_value, &member_cb, ""),
                btn_icon(&t("force_join.field_member"), &member_cb, ""),
            ],
            vec![btn_icon(&t("force_join.reserve_button"), &reserve_cb, "")],
            vec![btn_icon_plain("\u{200F}", CB_FJ_NOOP, "")],
            vec![btn_icon(&t("force_join.delete_button"), &delete_cb, "")],
            vec![btn_icon(
                &t("force_join.back_to_prev_button"),
                CB_FJ_VIEW,
                "back",
            )],
        ])
        .build();
    Some((text, kb))
}

/// Displays lock management panel by editing existing message.
pub async fn open_manage(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    lock_id: i64,
    database: &Option<PostgresDatabase>,
) {
    match build_manage(lock_id, database).await {
        Some((text, kb)) => edit_text_np(api, chat_id, message_id, &text, Some(kb)).await,
        None => {
            let kb = InlineKeyboardMarkup::builder()
                .inline_keyboard(vec![vec![btn_icon(&t("admin.back"), CB_FJ_VIEW, "back")]])
                .build();
            edit_text_np(
                api,
                chat_id,
                message_id,
                &t("force_join.lock_not_found"),
                Some(kb),
            )
            .await;
        }
    }
}

/// Lock management panel as a new message (after admin text input).
pub async fn send_manage(
    api: &Bot,
    chat_id: i64,
    lock_id: i64,
    database: &Option<PostgresDatabase>,
) {
    if let Some((text, kb)) = build_manage(lock_id, database).await {
        let params = SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .link_preview_options(no_preview())
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(kb))
            .build();
        let _ = api.send_message(&params).await;
    }
}

/// Displays delete confirmation menu for a lock.
pub async fn open_delete_confirm(api: &Bot, chat_id: i64, message_id: i32, lock_id: i64) {
    let name = get_lock(lock_id)
        .await
        .map(|l| l.display_name())
        .unwrap_or_default();
    let yes_cb = format!("{FJ_DELETE_YES_PREFIX}{lock_id}");
    let back_cb = format!("{FJ_MANAGE_PREFIX}{lock_id}");
    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon(&t("force_join.delete_confirm_yes"), &yes_cb, "")],
            vec![btn_icon(
                &t("force_join.delete_confirm_no"),
                &back_cb,
                "back",
            )],
        ])
        .build();
    edit_text_np(
        api,
        chat_id,
        message_id,
        &tf("force_join.delete_confirm_text", &[("name", &name)]),
        Some(kb),
    )
    .await;
}

/// Starts field edit wizard (name/time/member/reserve): turns panel message into input prompt
/// and awaits next text message.
pub async fn prompt_field(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    lock_id: i64,
    field: &str,
    flow_manager: &mut crate::emoji::FlowManager,
    user_id: i64,
) {
    flow_manager.set(
        user_id,
        crate::emoji::FlowState::AwaitingForceJoinField {
            lock_id,
            field: field.to_string(),
        },
    );
    let prompt_key = match field {
        "name" => "force_join.prompt_name",
        "time" => "force_join.prompt_time",
        "member" => "force_join.prompt_member",
        "reserve" => "force_join.prompt_reserve",
        _ => "force_join.prompt_name",
    };
    let back_cb = format!("{FJ_MANAGE_PREFIX}{lock_id}");
    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(&t("admin.back"), &back_cb, "back")]])
        .build();
    edit_text_np(api, chat_id, message_id, &t(prompt_key), Some(kb)).await;
}

/// Saves field edit wizard text input and re-displays management panel.
pub async fn handle_field_message(
    api: &Bot,
    chat_id: i64,
    lock_id: i64,
    field: &str,
    text: &str,
    flow_manager: &mut crate::emoji::FlowManager,
    user_id: i64,
    database: &Option<PostgresDatabase>,
) {
    let ok = match field {
        "name" => {
            set_display_name(lock_id, text).await;
            true
        }
        "time" => set_time_limit(lock_id, text).await,
        "member" => set_member_cap(lock_id, text).await,
        "reserve" => {
            set_reserve_link(lock_id, text).await;
            true
        }
        _ => false,
    };
    if !ok {
        // Invalid input (number required) — stay in wizard and show error.
        send_text_np(api, chat_id, &t("force_join.invalid_number")).await;
        return;
    }
    flow_manager.clear(user_id);
    // Send updated panel as new message under user message.
    send_manage(api, chat_id, lock_id, database).await;
}

/// "Add new lock" button: shows link format instructions and awaits next message.
pub async fn prompt_add_new(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    flow_manager: &mut crate::emoji::FlowManager,
    user_id: i64,
) {
    flow_manager.set(user_id, crate::emoji::FlowState::AwaitingForceJoinLink);
    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(
            &t("admin.back"),
            CB_FJ_ADD_CANCEL,
            "back",
        )]])
        .build();
    edit_text_np(
        api,
        chat_id,
        message_id,
        &t("force_join.add_prompt"),
        Some(kb),
    )
    .await;
}

pub(crate) fn is_private_tme_link(text: &str) -> bool {
    text.contains("t.me/+") || text.contains("t.me/joinchat/")
}

/// Called when admin sends text message in AwaitingForceJoinLink state.
pub async fn handle_link_message(
    api: &Bot,
    chat_id: i64,
    text: &str,
    flow_manager: &mut crate::emoji::FlowManager,
    user_id: i64,
) {
    if is_private_tme_link(text) {
        flow_manager.set(
            user_id,
            crate::emoji::FlowState::AwaitingForceJoinPrivateInfo {
                link: text.to_string(),
            },
        );
        send_text_np(api, chat_id, &t("force_join.private_link_saved")).await;
    } else {
        add_lock(api, text, None).await;
        flow_manager.clear(user_id);
        send_text_np(api, chat_id, &t("force_join.link_added_optional")).await;
        send_locks_list(api, chat_id).await;
    }
}

/// Called when admin sends username/numeric ID/forwarded message in AwaitingForceJoinPrivateInfo state.
pub async fn handle_private_info_message(
    api: &Bot,
    chat_id: i64,
    link: &str,
    message: &frankenstein::types::Message,
    flow_manager: &mut crate::emoji::FlowManager,
    user_id: i64,
) {
    use frankenstein::types::MessageOrigin;

    let identifier = if let Some(origin) = &message.forward_origin {
        match origin.as_ref() {
            MessageOrigin::Channel(o) => Some(
                o.chat
                    .username
                    .as_ref()
                    .map(|u| format!("@{u}"))
                    .unwrap_or_else(|| o.chat.id.to_string()),
            ),
            MessageOrigin::Chat(o) => Some(
                o.sender_chat
                    .username
                    .as_ref()
                    .map(|u| format!("@{u}"))
                    .unwrap_or_else(|| o.sender_chat.id.to_string()),
            ),
            _ => None,
        }
    } else if let Some(text) = message.text.as_deref() {
        let text = text.trim();
        if text.starts_with('@') {
            Some(text.to_string())
        } else if text.starts_with("-100") && text[1..].chars().all(|c| c.is_ascii_digit()) {
            Some(text.to_string())
        } else {
            None
        }
    } else {
        None
    };

    let Some(identifier) = identifier else { return }; // Unrecognized input; remain waiting

    add_lock(api, link, Some(&identifier)).await;
    flow_manager.clear(user_id);
    send_text_np(api, chat_id, &t("force_join.link_added_optional")).await;
    send_locks_list(api, chat_id).await;
}
