use axum::Json;
use frankenstein::client_reqwest::Bot;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::force_join::conn::conn;
use crate::force_join::{
    CB_FJ_ADD_NEW, CB_FJ_CHECK, CB_FJ_TOGGLE, CB_FJ_VIEW, Lock, ToggleModeResult, add_lock,
    already_count_key, build_manage, delete_lock, is_enabled, is_joined, is_joined_live,
    linked_count_key, locks_list_view, mandatory_locks, menu_keyboard, menu_text,
    set_display_name, set_enabled, set_field, set_member_cap, set_reserve_link, set_time_limit,
    toggle_lock_mode,
};
use crate::i18n::t;

// =========================================================================
// 1. /test/fj/gate
// =========================================================================

#[derive(Deserialize)]
pub struct FjGateReq {
    pub user_id: Option<i64>,
    pub enabled: Option<bool>,
    pub live: Option<bool>,
    pub simulated_membership: Option<String>, // "member", "left", "kicked", "error"
    pub setup_mandatory_lock: Option<bool>,
    pub is_check_btn: Option<bool>,
}

#[derive(Serialize)]
pub struct FjGateResp {
    pub ok: bool,
    pub allowed: bool,
    pub enabled: bool,
    pub mandatory_count: usize,
    pub is_check_btn: bool,
    pub rendered_locked_text: Option<String>,
    pub locked_keyboard: Option<Value>,
    pub check_button_cb: Option<String>,
    pub alert_toast: Option<String>,
    pub simulated_status: Option<String>,
}

pub async fn test_fj_gate(Json(req): Json<FjGateReq>) -> Json<FjGateResp> {
    let uid = req.user_id.unwrap_or(12345);
    let original_enabled = is_enabled().await;

    // Set enabled state if requested
    if let Some(en) = req.enabled {
        set_enabled(en).await;
    }

    // Set simulated chat member response in testapi state
    let sim_status = req.simulated_membership.clone();
    crate::testapi::state::set_simulated_chat_member(sim_status.as_deref());

    let bot = Bot::new_url("http://127.0.0.1:14379/bot".to_string());
    let mut temp_lock_id = None;

    if req.setup_mandatory_lock.unwrap_or(false) {
        let lid = add_lock(&bot, "https://t.me/test_gate_chan", None).await;
        if lid > 0 {
            set_field(lid, "mode", "mandatory").await;
            temp_lock_id = Some(lid);
        }
    }

    // Clear any prior cached join status for this test user across locks
    if let Ok(mut c) = conn().await {
        for lock in mandatory_locks().await {
            let _: Result<(), _> = redis::cmd("DEL")
                .arg(crate::force_join::joined_key(lock.id, uid))
                .query_async(&mut c)
                .await;
        }
    }

    let is_live = req.live.unwrap_or(false);
    let allowed = if is_live {
        is_joined_live(&bot, uid).await
    } else {
        is_joined(&bot, uid).await
    };

    let mandatory = mandatory_locks().await;
    let mandatory_count = mandatory.len();

    let is_check_btn = req.is_check_btn.unwrap_or(false);
    let mut rendered_locked_text = None;
    let mut locked_keyboard = None;
    let mut check_button_cb = None;
    let mut alert_toast = None;

    if !allowed {
        rendered_locked_text = Some(t("force_join.locked_message"));
        let (_title, kb) = locks_list_view(&mandatory);
        locked_keyboard = serde_json::to_value(&kb).ok();
        check_button_cb = Some(CB_FJ_CHECK.to_string());

        if is_check_btn {
            alert_toast = Some(t("force_join.still_not_joined"));
        }
    } else if is_check_btn {
        alert_toast = Some(t("force_join.now_joined"));
    }

    // Clean up temporary lock if created
    if let Some(lid) = temp_lock_id {
        delete_lock(lid).await;
    }

    // Clean up cached keys for uid
    if let Ok(mut c) = conn().await {
        for lock in &mandatory {
            let _: Result<(), _> = redis::cmd("DEL")
                .arg(crate::force_join::joined_key(lock.id, uid))
                .query_async(&mut c)
                .await;
        }
    }

    // Reset simulated member status & restore enabled
    crate::testapi::state::set_simulated_chat_member(None);
    if req.enabled.is_some() {
        set_enabled(original_enabled).await;
    }

    Json(FjGateResp {
        ok: true,
        allowed,
        enabled: is_enabled().await,
        mandatory_count,
        is_check_btn,
        rendered_locked_text,
        locked_keyboard,
        check_button_cb,
        alert_toast,
        simulated_status: sim_status,
    })
}

// =========================================================================
// 2. /test/fj/admin/menu
// =========================================================================

#[derive(Deserialize)]
pub struct FjAdminMenuReq {
    pub enabled: Option<bool>,
}

#[derive(Serialize)]
pub struct FjAdminMenuResp {
    pub ok: bool,
    pub enabled: bool,
    pub rendered_text: String,
    pub status_button_text: String,
    pub status_button_cb: String,
    pub view_button_text: String,
    pub view_button_cb: String,
    pub back_button_cb: String,
    pub keyboard: Value,
}

pub async fn test_fj_admin_menu(Json(req): Json<FjAdminMenuReq>) -> Json<FjAdminMenuResp> {
    let enabled = req.enabled.unwrap_or(true);
    let rendered_text = menu_text();
    let kb = menu_keyboard(enabled);

    let status_text = if enabled {
        t("force_join.status_on")
    } else {
        t("force_join.status_off")
    };

    let keyboard_val = serde_json::to_value(&kb).unwrap_or(json!([]));

    Json(fadmin_menu_resp_from(
        enabled,
        rendered_text,
        status_text,
        CB_FJ_TOGGLE.to_string(),
        t("force_join.view_button"),
        CB_FJ_VIEW.to_string(),
        crate::bot::CB_ADMIN_PANEL.to_string(),
        keyboard_val,
    ))
}

fn fadmin_menu_resp_from(
    enabled: bool,
    rendered_text: String,
    status_button_text: String,
    status_button_cb: String,
    view_button_text: String,
    view_button_cb: String,
    back_button_cb: String,
    keyboard: Value,
) -> FjAdminMenuResp {
    FjAdminMenuResp {
        ok: true,
        enabled,
        rendered_text,
        status_button_text,
        status_button_cb,
        view_button_text,
        view_button_cb,
        back_button_cb,
        keyboard,
    }
}

// =========================================================================
// 3. /test/fj/admin/locks
// =========================================================================

#[derive(Deserialize)]
pub struct FjAdminLocksReq {
    pub simulate_empty: Option<bool>,
    pub custom_locks: Option<Vec<FjLockItemReq>>,
}

#[derive(Deserialize)]
pub struct FjLockItemReq {
    pub id: i64,
    pub link: String,
    pub display_name: Option<String>,
    pub mode: Option<String>,
}

#[derive(Serialize)]
pub struct FjAdminLocksResp {
    pub ok: bool,
    pub is_empty: bool,
    pub lock_count: usize,
    pub rendered_title: String,
    pub manage_buttons: Vec<FjLockManageButton>,
    pub add_new_cb: String,
    pub back_cb: String,
    pub keyboard: Value,
}

#[derive(Serialize)]
pub struct FjLockManageButton {
    pub lock_id: i64,
    pub display_name: String,
    pub callback_data: String,
}

pub async fn test_fj_admin_locks(Json(req): Json<FjAdminLocksReq>) -> Json<FjAdminLocksResp> {
    let is_empty = req.simulate_empty.unwrap_or(false);

    let locks: Vec<Lock> = if is_empty {
        Vec::new()
    } else if let Some(custom) = req.custom_locks {
        custom
            .into_iter()
            .map(|c| Lock {
                id: c.id,
                link: c.link,
                identifier: "".to_string(),
                title: "".to_string(),
                display_override: c.display_name.unwrap_or_default(),
                mode: c.mode.unwrap_or_else(|| "optional".to_string()),
                created_at: 1700000000,
                expires_at: 0,
                member_cap: 0,
                reserve_link: "".to_string(),
            })
            .collect()
    } else {
        vec![
            Lock {
                id: 1,
                link: "https://t.me/channel_one".to_string(),
                identifier: "@channel_one".to_string(),
                title: "Channel One".to_string(),
                display_override: "Channel 1 Official".to_string(),
                mode: "mandatory".to_string(),
                created_at: 1700000000,
                expires_at: 0,
                member_cap: 0,
                reserve_link: "".to_string(),
            },
            Lock {
                id: 2,
                link: "https://t.me/channel_two".to_string(),
                identifier: "@channel_two".to_string(),
                title: "Channel Two".to_string(),
                display_override: "".to_string(),
                mode: "optional".to_string(),
                created_at: 1700000000,
                expires_at: 0,
                member_cap: 0,
                reserve_link: "".to_string(),
            },
        ]
    };

    let lock_count = locks.len();
    let (rendered_title, kb) = locks_list_view(&locks);

    let mut manage_buttons = Vec::new();
    for row in &kb.inline_keyboard {
        for btn in row {
            if let Some(cb) = &btn.callback_data {
                if cb.starts_with(crate::force_join::FJ_MANAGE_PREFIX) {
                    let id_str = cb.trim_start_matches(crate::force_join::FJ_MANAGE_PREFIX);
                    if let Ok(lid) = id_str.parse::<i64>() {
                        manage_buttons.push(FjLockManageButton {
                            lock_id: lid,
                            display_name: btn.text.clone(),
                            callback_data: cb.clone(),
                        });
                    }
                }
            }
        }
    }

    let keyboard_val = serde_json::to_value(&kb).unwrap_or(json!([]));

    Json(FjAdminLocksResp {
        ok: true,
        is_empty: lock_count == 0,
        lock_count,
        rendered_title,
        manage_buttons,
        add_new_cb: CB_FJ_ADD_NEW.to_string(),
        back_cb: crate::bot::CB_ADMIN_FORCE_JOIN.to_string(),
        keyboard: keyboard_val,
    })
}

// =========================================================================
// 4. /test/fj/admin/manage
// =========================================================================

#[derive(Deserialize)]
pub struct FjAdminManageReq {
    pub lock_id: Option<i64>,
    pub link: Option<String>,
    pub identifier: Option<String>,
    pub mode: Option<String>,
    pub member_cap: Option<i64>,
    pub expires_in_days: Option<i64>,
    pub reserve_link: Option<String>,
    pub display_override: Option<String>,
    pub already_joined: Option<i64>,
    pub joined_via_link: Option<i64>,
}

#[derive(Serialize)]
pub struct FjAdminManageResp {
    pub ok: bool,
    pub lock_found: bool,
    pub lock_id: i64,
    pub rendered_text: String,
    pub stats: FjLockStatsResp,
    pub buttons: FjManageButtonsResp,
    pub keyboard: Value,
}

#[derive(Serialize)]
pub struct FjLockStatsResp {
    pub total_users: i64,
    pub joined_via_link: i64,
    pub already_joined: i64,
    pub not_joined: i64,
}

#[derive(Serialize)]
pub struct FjManageButtonsResp {
    pub name_cb: String,
    pub mode_cb: String,
    pub mode_value: String,
    pub time_cb: String,
    pub time_value: String,
    pub member_cb: String,
    pub member_value: String,
    pub reserve_cb: String,
    pub delete_cb: String,
    pub back_cb: String,
}

pub async fn test_fj_admin_manage(Json(req): Json<FjAdminManageReq>) -> Json<FjAdminManageResp> {
    let bot = Bot::new_url("http://127.0.0.1:14379/bot".to_string());
    let link = req
        .link
        .unwrap_or_else(|| "https://t.me/test_manage_channel".to_string());
    let lock_id = match req.lock_id {
        Some(id) => id,
        None => add_lock(&bot, &link, req.identifier.as_deref()).await,
    };

    if let Some(name) = &req.display_override {
        set_display_name(lock_id, name).await;
    }
    if let Some(m) = &req.mode {
        set_field(lock_id, "mode", m).await;
    }
    if let Some(days) = req.expires_in_days {
        set_time_limit(lock_id, &days.to_string()).await;
    }
    if let Some(cap) = req.member_cap {
        set_member_cap(lock_id, &cap.to_string()).await;
    }
    if let Some(res) = &req.reserve_link {
        set_reserve_link(lock_id, res).await;
    }

    // Set stats in Redis if given
    let already = req.already_joined.unwrap_or(15);
    let linked = req.joined_via_link.unwrap_or(25);

    let mut c = conn().await.ok();
    if let Some(conn) = c.as_mut() {
        let _: Result<(), _> = redis::cmd("SET")
            .arg(already_count_key(lock_id))
            .arg(already.to_string())
            .query_async(conn)
            .await;
        let _: Result<(), _> = redis::cmd("SET")
            .arg(linked_count_key(lock_id))
            .arg(linked.to_string())
            .query_async(conn)
            .await;
    }

    let database = crate::testapi::state::db().await.clone();
    let manage_view = build_manage(lock_id, &database).await;

    let (rendered_text, kb) = match manage_view {
        Some((t, k)) => (t, k),
        None => ("Not Found".to_string(), menu_keyboard(false)),
    };

    let keyboard_val = serde_json::to_value(&kb).unwrap_or(json!([]));

    let lock = crate::force_join::get_lock(lock_id).await;
    let lock_found = lock.is_some();

    // Clean up temporary lock
    delete_lock(lock_id).await;

    let name_cb = format!("{}{}", crate::force_join::FJ_NAME_PREFIX, lock_id);
    let mode_cb = format!("{}{}", crate::force_join::FJ_MODE_PREFIX, lock_id);
    let time_cb = format!("{}{}", crate::force_join::FJ_TIME_PREFIX, lock_id);
    let member_cb = format!("{}{}", crate::force_join::FJ_MEMBER_PREFIX, lock_id);
    let reserve_cb = format!("{}{}", crate::force_join::FJ_RESERVE_PREFIX, lock_id);
    let delete_cb = format!("{}{}", crate::force_join::FJ_DELETE_PREFIX, lock_id);

    Json(FjAdminManageResp {
        ok: true,
        lock_found,
        lock_id,
        rendered_text,
        stats: FjLockStatsResp {
            total_users: 0,
            joined_via_link: linked,
            already_joined: already,
            not_joined: 0,
        },
        buttons: FjManageButtonsResp {
            name_cb,
            mode_cb,
            mode_value: if req.mode.as_deref() == Some("mandatory") {
                t("force_join.mode_mandatory")
            } else {
                t("force_join.mode_optional")
            },
            time_cb,
            time_value: if req.expires_in_days.is_some() {
                "expires".to_string()
            } else {
                t("force_join.limit_none")
            },
            member_cb,
            member_value: if let Some(cap) = req.member_cap {
                cap.to_string()
            } else {
                t("force_join.limit_none")
            },
            reserve_cb,
            delete_cb,
            back_cb: CB_FJ_VIEW.to_string(),
        },
        keyboard: keyboard_val,
    })
}

// =========================================================================
// 5. /test/fj/admin/toggle_mode
// =========================================================================

#[derive(Deserialize)]
pub struct FjToggleModeReq {
    pub scenario: String, // "ok", "bot_not_admin", "no_chat_id", "not_found", "toggle_to_optional"
    pub bot_is_admin: Option<bool>,
}

#[derive(Serialize)]
pub struct FjToggleModeResp {
    pub ok: bool,
    pub scenario: String,
    pub result: String, // "Ok", "BotNotAdmin", "NoChatId", "NotFound"
    pub is_error: bool,
    pub error_message: Option<String>,
    pub resulting_mode: Option<String>,
}

pub async fn test_fj_admin_toggle_mode(
    Json(req): Json<FjToggleModeReq>,
) -> Json<FjToggleModeResp> {
    let bot = Bot::new_url("http://127.0.0.1:14379/bot".to_string());

    let (res, resulting_mode) = match req.scenario.as_str() {
        "not_found" => {
            let res = toggle_lock_mode(&bot, -99999).await;
            (res, None)
        }
        "no_chat_id" => {
            let lid = add_lock(&bot, "https://instagram.com/no_tg", None).await;
            set_field(lid, "identifier", "").await;
            set_field(lid, "mode", "optional").await;
            let res = toggle_lock_mode(&bot, lid).await;
            let final_mode = crate::force_join::get_lock(lid).await.map(|l| l.mode);
            delete_lock(lid).await;
            (res, final_mode)
        }
        "bot_not_admin" => {
            // Set mock bot admin status to false
            crate::testapi::state::set_simulated_bot_admin(Some(false));
            let lid = add_lock(&bot, "https://t.me/test_not_admin_channel", None).await;
            set_field(lid, "mode", "optional").await;
            let res = toggle_lock_mode(&bot, lid).await;
            let final_mode = crate::force_join::get_lock(lid).await.map(|l| l.mode);
            delete_lock(lid).await;
            crate::testapi::state::set_simulated_bot_admin(None);
            (res, final_mode)
        }
        "toggle_to_optional" => {
            let lid = add_lock(&bot, "https://t.me/test_mand_toggle_opt", None).await;
            set_field(lid, "mode", "mandatory").await;
            let res = toggle_lock_mode(&bot, lid).await;
            let final_mode = crate::force_join::get_lock(lid).await.map(|l| l.mode);
            delete_lock(lid).await;
            (res, final_mode)
        }
        _ => {
            // "ok": bot is admin, valid chat ID, optional -> mandatory
            let is_admin = req.bot_is_admin.unwrap_or(true);
            crate::testapi::state::set_simulated_bot_admin(Some(is_admin));
            let lid = add_lock(&bot, "https://t.me/test_admin_ok_channel", None).await;
            set_field(lid, "mode", "optional").await;
            let res = toggle_lock_mode(&bot, lid).await;
            let final_mode = crate::force_join::get_lock(lid).await.map(|l| l.mode);
            delete_lock(lid).await;
            crate::testapi::state::set_simulated_bot_admin(None);
            (res, final_mode)
        }
    };

    let (result_str, is_error, error_message) = match res {
        ToggleModeResult::Ok => ("Ok".to_string(), false, None),
        ToggleModeResult::BotNotAdmin => (
            "BotNotAdmin".to_string(),
            true,
            Some(t("force_join.bot_not_admin")),
        ),
        ToggleModeResult::NoChatId => (
            "NoChatId".to_string(),
            true,
            Some(t("force_join.no_chat_id")),
        ),
        ToggleModeResult::NotFound => ("NotFound".to_string(), true, Some("Not Found".to_string())),
    };

    Json(FjToggleModeResp {
        ok: true,
        scenario: req.scenario,
        result: result_str,
        is_error,
        error_message,
        resulting_mode,
    })
}
