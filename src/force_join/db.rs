use std::collections::HashMap;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::GetChatParams,
    types::ChatId,
};

use crate::force_join::cache::linked_count;
use crate::force_join::conn::{
    ENABLED_KEY, LOCK_IDS_KEY, NEXT_ID_KEY, already_count_key, conn, linked_count_key,
    lock_hash_key,
};
use crate::force_join::jalali::{now_epoch, to_en_digits};
use crate::force_join::types::{Lock, chat_id_for};

/// Extracts chat identifier from a public `t.me/username` link; returns None for
/// private (`+`) or non-Telegram links.
pub(crate) fn derive_identifier(link: &str) -> Option<String> {
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
pub(crate) async fn fetch_chat_title(api: &Bot, chat_id: ChatId) -> String {
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

pub async fn get_lock(id: i64) -> Option<Lock> {
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

pub async fn set_field(id: i64, field: &str, value: &str) {
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

pub async fn list_locks() -> Vec<Lock> {
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
pub async fn mandatory_locks() -> Vec<Lock> {
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

pub async fn set_enabled(enabled: bool) {
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
