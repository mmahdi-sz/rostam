//! Multiple mandatory/optional membership locks stored in Redis (Hash).
//!
//! Each lock: numeric id, link, chat identifier (`@username` or numeric ID — for membership check via
//! `getChatMember`), mode (`mandatory`/`optional`). Only mandatory identifiable locks
//! (valid chat identifier) are checked; optional ones display their link without membership verification.

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, GetChatMemberParams, GetChatParams, SendMessageParams},
    types::{
        Chat, ChatId, ChatMember, InlineKeyboardButton, InlineKeyboardMarkup, LinkPreviewOptions,
        ReplyMarkup,
    },
};
use redis::aio::MultiplexedConnection;
use tokio::sync::OnceCell;

static REDIS_CONN: OnceCell<MultiplexedConnection> = OnceCell::const_new();

async fn conn() -> redis::RedisResult<MultiplexedConnection> {
    REDIS_CONN
        .get_or_try_init(|| async {
            let client = redis::Client::open(config::redis_url())?;
            client.get_multiplexed_async_connection().await
        })
        .await
        .cloned()
}
use std::collections::HashMap;

use crate::config;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::panel::{btn_icon, btn_icon_plain, btn_icon_success};
use crate::i18n::{t, tf};
use crate::youtube::jalali::gregorian_to_jalali;

const JOINED_TTL_SECS: u64 = 86400 * 30;
const NOT_JOINED_TTL_SECS: u64 = 300;
const ENABLED_KEY: &str = "force_join:enabled";
const NEXT_ID_KEY: &str = "force_join:next_id";
const LOCK_IDS_KEY: &str = "force_join:lock_ids";

// Admin panel — "Force join lock" submenu buttons
pub const CB_FJ_TOGGLE: &str = "fj:toggle";
pub const CB_FJ_VIEW: &str = "fj:view";
pub const CB_FJ_NOOP: &str = "fj:nop";
pub const CB_FJ_ADD_NEW: &str = "fj:add";
pub const CB_FJ_ADD_CANCEL: &str = "fj:add_cancel";
pub const CB_FJ_CHECK: &str = "fj:check";
pub const FJ_MANAGE_PREFIX: &str = "fj:manage:";
pub const FJ_MODE_PREFIX: &str = "fj:mode:";
pub const FJ_NAME_PREFIX: &str = "fj:name:";
pub const FJ_TIME_PREFIX: &str = "fj:time:";
pub const FJ_MEMBER_PREFIX: &str = "fj:member:";
pub const FJ_RESERVE_PREFIX: &str = "fj:reserve:";
pub const FJ_DELETE_PREFIX: &str = "fj:delete:";
pub const FJ_DELETE_YES_PREFIX: &str = "fj:del_yes:";

pub struct Lock {
    pub id: i64,
    pub link: String,
    pub identifier: String,
    pub title: String,
    pub display_override: String,
    pub mode: String,
    pub created_at: i64,
    pub expires_at: i64, // 0 = no time limit
    pub member_cap: i64, // 0 = no member limit
    pub reserve_link: String,
}

impl Lock {
    /// Display name: Admin manual override, else chat title (getChat), else identifier, else link.
    fn display_name(&self) -> String {
        if !self.display_override.is_empty() {
            self.display_override.clone()
        } else if !self.title.is_empty() {
            self.title.clone()
        } else if !self.identifier.is_empty() {
            self.identifier.clone()
        } else {
            self.link.clone()
        }
    }

    fn is_mandatory(&self) -> bool {
        self.mode == "mandatory"
    }

    fn chat_id(&self) -> Option<ChatId> {
        chat_id_for(&self.identifier)
    }

    fn is_expired(&self) -> bool {
        self.expires_at != 0 && now_epoch() >= self.expires_at
    }
}

fn chat_id_for(identifier: &str) -> Option<ChatId> {
    if identifier.is_empty() {
        None
    } else if let Ok(n) = identifier.parse::<i64>() {
        Some(ChatId::Integer(n))
    } else if identifier.starts_with('@') {
        Some(ChatId::String(identifier.to_string()))
    } else {
        None
    }
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Persian/Arabic digits → English, for parsing admin numeric input.
fn to_en_digits(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '۰'..='۹' => ((c as u32 - '۰' as u32) as u8 + b'0') as char,
            '٠'..='٩' => ((c as u32 - '٠' as u32) as u8 + b'0') as char,
            other => other,
        })
        .collect()
}

/// Jalali date/time in Tehran timezone with English digits via chrono + gregorian_to_jalali.
fn fmt_jalali_dt(epoch: i64) -> String {
    use chrono::{Datelike, Timelike};
    use chrono_tz::Asia::Tehran;
    let Some(utc) = chrono::DateTime::from_timestamp(epoch, 0) else {
        return "—".to_string();
    };
    let dt = utc.with_timezone(&Tehran);
    let (jy, jm, jd) = gregorian_to_jalali(dt.year(), dt.month() as i32, dt.day() as i32);
    format!(
        "🗓 {jy}/{jm:02}/{jd:02} ⏰ {:02}:{:02}",
        dt.hour(),
        dt.minute()
    )
}

fn no_preview() -> LinkPreviewOptions {
    LinkPreviewOptions::builder().is_disabled(true).build()
}

/// Like `crate::bot::edit_text` but always disables link preview —
/// locks display raw link text and should not generate previews.
async fn edit_text_np(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    text: &str,
    kb: Option<InlineKeyboardMarkup>,
) {
    let _ = match kb {
        Some(kb) => {
            api.edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(message_id)
                    .text(text)
                    .link_preview_options(no_preview())
                    .reply_markup(kb)
                    .build(),
            )
            .await
        }
        None => {
            api.edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(message_id)
                    .text(text)
                    .link_preview_options(no_preview())
                    .build(),
            )
            .await
        }
    };
}

/// Like `crate::bot::send_text` but always disables link preview.
async fn send_text_np(api: &Bot, chat_id: i64, text: &str) {
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .link_preview_options(no_preview())
        .build();
    let _ = api.send_message(&params).await;
}

fn is_member_status(m: &ChatMember) -> bool {
    matches!(
        m,
        ChatMember::Creator(_)
            | ChatMember::Administrator(_)
            | ChatMember::Member(_)
            | ChatMember::Restricted(_)
    )
}

fn chat_member_user_id(m: &ChatMember) -> i64 {
    (match m {
        ChatMember::Creator(x) => x.user.id,
        ChatMember::Administrator(x) => x.user.id,
        ChatMember::Member(x) => x.user.id,
        ChatMember::Restricted(x) => x.user.id,
        ChatMember::Left(x) => x.user.id,
        ChatMember::Kicked(x) => x.user.id,
    }) as i64
}

fn lock_hash_key(id: i64) -> String {
    format!("force_join:lock:{id}")
}
fn joined_key(lock_id: i64, user_id: i64) -> String {
    format!("force_join:joined:{lock_id}:{user_id}")
}
fn counted_key(lock_id: i64, user_id: i64) -> String {
    format!("force_join:counted:{lock_id}:{user_id}")
}
fn already_count_key(lock_id: i64) -> String {
    format!("force_join:lock:{lock_id}:already_count")
}
fn linked_count_key(lock_id: i64) -> String {
    format!("force_join:lock:{lock_id}:linked_count")
}

/// Extracts chat identifier from a public `t.me/username` link; returns None for
/// private (`+`) or non-Telegram links.
fn derive_identifier(link: &str) -> Option<String> {
    if !link.contains("t.me/") {
        return None;
    }
    let seg = link.rsplit('/').next()?;
    if seg.is_empty() || seg.starts_with('+') {
        return None;
    }
    Some(format!("@{seg}"))
}

/// Gets actual chat title from Telegram (not username) — e.g. "Vilix", not "@vilix".
async fn fetch_chat_title(api: &Bot, chat_id: ChatId) -> String {
    let params = GetChatParams::builder().chat_id(chat_id).build();
    match api.get_chat(&params).await {
        Ok(resp) => resp.result.title.unwrap_or_default(),
        Err(_) => String::new(),
    }
}

/// Adds a new lock (default: optional). Chat identifier extracted from link if omitted.
pub async fn add_lock(api: &Bot, link: &str, identifier: Option<&str>) -> i64 {
    let resolved = identifier
        .map(|s| s.to_string())
        .or_else(|| derive_identifier(link))
        .unwrap_or_default();
    let title = match chat_id_for(&resolved) {
        Some(chat_id) => fetch_chat_title(api, chat_id).await,
        None => String::new(),
    };

    let Ok(mut c) = conn().await else { return 0 };
    let id: i64 = redis::cmd("INCR")
        .arg(NEXT_ID_KEY)
        .query_async(&mut c)
        .await
        .unwrap_or(0);
    let _: Result<(), _> = redis::cmd("HSET")
        .arg(lock_hash_key(id))
        .arg("link")
        .arg(link)
        .arg("identifier")
        .arg(&resolved)
        .arg("title")
        .arg(&title)
        .arg("mode")
        .arg("optional")
        .arg("created_at")
        .arg(now_epoch())
        .query_async::<()>(&mut c)
        .await;
    let _: Result<i64, _> = redis::cmd("RPUSH")
        .arg(LOCK_IDS_KEY)
        .arg(id)
        .query_async(&mut c)
        .await;
    id
}

async fn get_lock(id: i64) -> Option<Lock> {
    let mut c = conn().await.ok()?;
    let map: HashMap<String, String> = redis::cmd("HGETALL")
        .arg(lock_hash_key(id))
        .query_async(&mut c)
        .await
        .ok()?;
    if map.is_empty() {
        return None;
    }
    Some(Lock {
        id,
        link: map.get("link").cloned().unwrap_or_default(),
        identifier: map.get("identifier").cloned().unwrap_or_default(),
        title: map.get("title").cloned().unwrap_or_default(),
        display_override: map.get("display_override").cloned().unwrap_or_default(),
        mode: map
            .get("mode")
            .cloned()
            .unwrap_or_else(|| "optional".to_string()),
        created_at: map
            .get("created_at")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        expires_at: map
            .get("expires_at")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        member_cap: map
            .get("member_cap")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        reserve_link: map.get("reserve_link").cloned().unwrap_or_default(),
    })
}

async fn set_field(id: i64, field: &str, value: &str) {
    let Ok(mut c) = conn().await else { return };
    let _: Result<(), _> = redis::cmd("HSET")
        .arg(lock_hash_key(id))
        .arg(field)
        .arg(value)
        .query_async::<()>(&mut c)
        .await;
}

/// Deletes a lock completely (from list + hash + counters).
pub async fn delete_lock(id: i64) {
    let Ok(mut c) = conn().await else { return };
    let _: Result<i64, _> = redis::cmd("LREM")
        .arg(LOCK_IDS_KEY)
        .arg(0)
        .arg(id)
        .query_async(&mut c)
        .await;
    let _: Result<i64, _> = redis::cmd("DEL")
        .arg(lock_hash_key(id))
        .arg(already_count_key(id))
        .arg(linked_count_key(id))
        .query_async(&mut c)
        .await;
}

/// Saves custom admin display name.
pub async fn set_display_name(id: i64, name: &str) {
    set_field(id, "display_override", name.trim()).await;
}

/// Time limit: input number of days (e.g. "7" or "30"); "0"/"-"/"delete" → no limit.
/// Returns true if input was valid.
pub async fn set_time_limit(id: i64, input: &str) -> bool {
    let s = to_en_digits(input.trim());
    if s == "0" || s == "-" || s.contains("حذف") {
        set_field(id, "expires_at", "0").await;
        return true;
    }
    match s.parse::<i64>() {
        Ok(days) if days > 0 => {
            set_field(id, "expires_at", &(now_epoch() + days * 86400).to_string()).await;
            true
        }
        _ => false,
    }
}

/// Member cap: max members for this lock; "0"/"-"/"delete" → no limit.
pub async fn set_member_cap(id: i64, input: &str) -> bool {
    let s = to_en_digits(input.trim());
    if s == "0" || s == "-" || s.contains("حذف") {
        set_field(id, "member_cap", "0").await;
        return true;
    }
    match s.parse::<i64>() {
        Ok(cap) if cap > 0 => {
            set_field(id, "member_cap", &cap.to_string()).await;
            true
        }
        _ => false,
    }
}

/// Reserve link (fallback for this lock).
pub async fn set_reserve_link(id: i64, link: &str) {
    set_field(id, "reserve_link", link.trim()).await;
}

async fn linked_count(lock_id: i64) -> i64 {
    let Ok(mut c) = conn().await else { return 0 };
    redis::cmd("GET")
        .arg(linked_count_key(lock_id))
        .query_async(&mut c)
        .await
        .ok()
        .flatten()
        .unwrap_or(0)
}

async fn list_locks() -> Vec<Lock> {
    let Ok(mut c) = conn().await else {
        return Vec::new();
    };
    let ids: Vec<i64> = redis::cmd("LRANGE")
        .arg(LOCK_IDS_KEY)
        .arg(0)
        .arg(-1)
        .query_async(&mut c)
        .await
        .unwrap_or_default();
    let mut out = Vec::new();
    for id in ids {
        if let Some(l) = get_lock(id).await {
            out.push(l);
        }
    }
    out
}

/// Active mandatory locks: mandatory + valid identifier + not expired + member cap not reached.
async fn mandatory_locks() -> Vec<Lock> {
    let mut out = Vec::new();
    for l in list_locks().await {
        if !l.is_mandatory() || l.chat_id().is_none() || l.is_expired() {
            continue;
        }
        if l.member_cap != 0 && linked_count(l.id).await >= l.member_cap {
            continue; // Member cap reached → stop enforcing
        }
        out.push(l);
    }
    out
}

/// Bot must be chat admin (with full access) to enforce mandatory lock —
/// otherwise getChatMember fails for other users.
async fn bot_has_full_access(api: &Bot, chat_id: ChatId) -> bool {
    let Ok(me) = api.get_me().await else {
        return false;
    };
    let params = GetChatMemberParams::builder()
        .chat_id(chat_id)
        .user_id(me.result.id)
        .build();
    match api.get_chat_member(&params).await {
        Ok(resp) => matches!(
            resp.result,
            ChatMember::Administrator(_) | ChatMember::Creator(_)
        ),
        Err(_) => false,
    }
}

pub enum ToggleModeResult {
    Ok,
    /// Lock lacks identifiable chat ID (e.g. Instagram link) — cannot be mandatory.
    NoChatId,
    /// Bot is not admin in the channel/group.
    BotNotAdmin,
    NotFound,
}

pub async fn toggle_lock_mode(api: &Bot, id: i64) -> ToggleModeResult {
    let Some(lock) = get_lock(id).await else {
        return ToggleModeResult::NotFound;
    };

    let new_mode = if lock.is_mandatory() {
        "optional"
    } else {
        let Some(chat_id) = lock.chat_id() else {
            return ToggleModeResult::NoChatId;
        };
        if !bot_has_full_access(api, chat_id).await {
            return ToggleModeResult::BotNotAdmin;
        }
        "mandatory"
    };

    let Ok(mut c) = conn().await else {
        return ToggleModeResult::NotFound;
    };
    let _: Result<(), _> = redis::cmd("HSET")
        .arg(lock_hash_key(id))
        .arg("mode")
        .arg(new_mode)
        .query_async::<()>(&mut c)
        .await;
    ToggleModeResult::Ok
}

/// Caches user membership status for a lock and updates "already joined" /
/// "joined via link" counters.
pub async fn cache_status(lock_id: i64, user_id: i64, joined: bool) {
    let Ok(mut c) = conn().await else { return };
    let ttl = if joined {
        JOINED_TTL_SECS
    } else {
        NOT_JOINED_TTL_SECS
    };
    let _: Result<(), _> = redis::cmd("SET")
        .arg(joined_key(lock_id, user_id))
        .arg(if joined { "1" } else { "0" })
        .arg("EX")
        .arg(ttl)
        .query_async::<()>(&mut c)
        .await;

    let ckey = counted_key(lock_id, user_id);
    let script = r#"
        local prev = redis.call('GET', KEYS[1])
        local joined = ARGV[1]
        local already_key = KEYS[2]
        local linked_key = KEYS[3]
        if not prev then
            if joined == '1' then
                redis.call('SET', KEYS[1], 'already')
                redis.call('INCR', already_key)
            else
                redis.call('SET', KEYS[1], 'pending')
            end
        elseif prev == 'pending' and joined == '1' then
            redis.call('SET', KEYS[1], 'linked')
            redis.call('INCR', linked_key)
        elseif prev == 'linked' and joined == '0' then
            redis.call('SET', KEYS[1], 'pending')
            redis.call('DECR', linked_key)
        end
    "#;
    let joined_arg = if joined { "1" } else { "0" };
    let _: Result<(), _> = redis::cmd("EVAL")
        .arg(script)
        .arg(3)
        .arg(&ckey)
        .arg(already_count_key(lock_id))
        .arg(linked_count_key(lock_id))
        .arg(joined_arg)
        .query_async(&mut c)
        .await;
}

async fn check_lock_membership(api: &Bot, lock: &Lock, user_id: i64, force: bool) -> bool {
    if !force {
        let Ok(mut c) = conn().await else { return true };
        let jkey = joined_key(lock.id, user_id);
        let cached: Option<String> = redis::cmd("GET")
            .arg(&jkey)
            .query_async(&mut c)
            .await
            .ok()
            .flatten();
        if let Some(v) = cached {
            return v == "1";
        }
    }

    let Some(chat_id) = lock.chat_id() else {
        return true;
    };
    let params = GetChatMemberParams::builder()
        .chat_id(chat_id)
        .user_id(user_id as u64)
        .build();
    let joined = match api.get_chat_member(&params).await {
        Ok(resp) => is_member_status(&resp.result),
        Err(_) => true, // Missing info should not lock out user
    };
    cache_status(lock.id, user_id, joined).await;
    joined
}

async fn is_joined_inner(api: &Bot, user_id: i64, force: bool) -> bool {
    if !is_enabled().await {
        return true;
    }
    let mandatory = mandatory_locks().await;
    if mandatory.is_empty() {
        return true;
    }
    for lock in &mandatory {
        if !check_lock_membership(api, lock, user_id, force).await {
            return false;
        }
    }
    true
}

/// Checks all identifiable mandatory locks (cached). Always true if master toggle is off
/// or no mandatory locks exist.
pub async fn is_joined(api: &Bot, user_id: i64) -> bool {
    is_joined_inner(api, user_id, false).await
}

/// Like `is_joined` but bypasses cache and queries Telegram directly —
/// for "I joined" button so it is not delayed by `NOT_JOINED_TTL_SECS`.
pub async fn is_joined_live(api: &Bot, user_id: i64) -> bool {
    is_joined_inner(api, user_id, true).await
}

/// Maps `chat_member` update to matching lock (may match multiple locks).
pub async fn on_chat_member_update(chat: &Chat, new_member: &ChatMember) {
    let joined = is_member_status(new_member);
    let user_id = chat_member_user_id(new_member);
    let username_at = chat.username.as_deref().map(|u| format!("@{u}"));

    for lock in list_locks().await {
        let matches = match lock.chat_id() {
            Some(ChatId::Integer(n)) => n == chat.id,
            Some(ChatId::String(s)) => Some(&s) == username_at.as_ref(),
            _ => false,
        };
        if matches {
            cache_status(lock.id, user_id, joined).await;
        }
    }
}

/// Global mandatory lock toggle switch (independent of individual locks).
pub async fn is_enabled() -> bool {
    let Ok(mut c) = conn().await else {
        return false;
    };
    let v: Option<String> = redis::cmd("GET")
        .arg(ENABLED_KEY)
        .query_async(&mut c)
        .await
        .ok()
        .flatten();
    v.as_deref() == Some("1")
}

async fn set_enabled(enabled: bool) {
    let Ok(mut c) = conn().await else { return };
    let _: Result<(), _> = redis::cmd("SET")
        .arg(ENABLED_KEY)
        .arg(if enabled { "1" } else { "0" })
        .query_async::<()>(&mut c)
        .await;
}

pub async fn toggle_enabled() -> bool {
    let new_state = !is_enabled().await;
    set_enabled(new_state).await;
    new_state
}

fn menu_text() -> String {
    let username = config::bot_username();
    let at_username = if username.is_empty() {
        String::new()
    } else {
        format!("@{username}")
    };
    tf("force_join.info_text", &[("bot_username", &at_username)])
}

fn menu_keyboard(enabled: bool) -> InlineKeyboardMarkup {
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

fn locks_list_view(locks: &[Lock]) -> (String, InlineKeyboardMarkup) {
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
async fn build_manage(
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
        Some(db) => crate::stats::get_user_stats(db.client())
            .await
            .map(|s| s.total)
            .unwrap_or(0),
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

fn is_private_tme_link(text: &str) -> bool {
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

fn url_button(text: &str, url: &str) -> InlineKeyboardButton {
    InlineKeyboardButton {
        text: text.to_string(),
        icon_custom_emoji_id: None,
        callback_data: None,
        style: None,
        url: Some(url.to_string()),
        login_url: None,
        web_app: None,
        switch_inline_query: None,
        switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None,
        copy_text: None,
        callback_game: None,
        pay: None,
    }
}

/// Lock message: all locks (mandatory + optional) rendered as link buttons.
/// Reserve link (if present) rendered in same row next to main link.
pub async fn send_lock_message(api: &Bot, chat_id: i64) {
    let locks = list_locks().await;
    let mut rows: Vec<Vec<InlineKeyboardButton>> = locks
        .iter()
        .map(|l| {
            let mut row = vec![url_button(&l.display_name(), &l.link)];
            if !l.reserve_link.is_empty() {
                row.push(url_button(
                    &t("force_join.reserve_link_label"),
                    &l.reserve_link,
                ));
            }
            row
        })
        .collect();
    rows.push(vec![btn_icon_success(
        &t("force_join.check_button"),
        CB_FJ_CHECK,
        "check",
    )]);
    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build();
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(t("force_join.locked_message"))
        .link_preview_options(no_preview())
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(kb))
        .build();
    let _ = api.send_message(&params).await;
}
