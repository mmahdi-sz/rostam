use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::GetChatMemberParams,
    types::{ChatId, ChatMember},
};

use crate::force_join::cache::{cache_status, is_member_status};
use crate::force_join::conn::{conn, joined_key, lock_hash_key};
use crate::force_join::db::{get_lock, is_enabled, mandatory_locks};
use crate::force_join::types::{Lock, ToggleModeResult};

/// Bot must be chat admin (with full access) to enforce mandatory lock —
/// otherwise getChatMember fails for other users.
pub async fn bot_has_full_access(api: &Bot, chat_id: ChatId) -> bool {
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

pub async fn check_lock_membership(api: &Bot, lock: &Lock, user_id: i64, force: bool) -> bool {
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
