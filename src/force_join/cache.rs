use frankenstein::types::{Chat, ChatId, ChatMember};

use crate::force_join::conn::{
    JOINED_TTL_SECS, NOT_JOINED_TTL_SECS, already_count_key, conn, counted_key, joined_key,
    linked_count_key,
};
use crate::force_join::db::list_locks;

pub(crate) fn is_member_status(m: &ChatMember) -> bool {
    matches!(
        m,
        ChatMember::Creator(_)
            | ChatMember::Administrator(_)
            | ChatMember::Member(_)
            | ChatMember::Restricted(_)
    )
}

pub(crate) fn chat_member_user_id(m: &ChatMember) -> i64 {
    (match m {
        ChatMember::Creator(x) => x.user.id,
        ChatMember::Administrator(x) => x.user.id,
        ChatMember::Member(x) => x.user.id,
        ChatMember::Restricted(x) => x.user.id,
        ChatMember::Left(x) => x.user.id,
        ChatMember::Kicked(x) => x.user.id,
    }) as i64
}

pub(crate) async fn linked_count(lock_id: i64) -> i64 {
    let Ok(mut c) = conn().await else { return 0 };
    redis::cmd("GET")
        .arg(linked_count_key(lock_id))
        .query_async(&mut c)
        .await
        .ok()
        .flatten()
        .unwrap_or(0)
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
