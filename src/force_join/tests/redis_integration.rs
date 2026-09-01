use frankenstein::client_reqwest::Bot;

use crate::force_join::cache::{cache_status, linked_count};
use crate::force_join::conn::{
    already_count_key, conn, counted_key, joined_key, linked_count_key, lock_hash_key,
};
use crate::force_join::db::{
    add_lock, delete_lock, get_lock, list_locks, mandatory_locks, set_display_name, set_field,
    set_member_cap, set_reserve_link, set_time_limit,
};
use crate::force_join::jalali::now_epoch;

// =========================================================================
// Phase 2: Redis / Lua Script Integration Tests (Real dev Redis connection)
// Run with: cargo test force_join -- --ignored
// =========================================================================

/// 1. Lock CRUD Lifecycle (Risk: HIGH)
/// Full lifecycle: create lock -> retrieve -> update fields -> list -> delete -> verify purged.
#[tokio::test]
#[ignore]
async fn test_lock_crud_lifecycle() {
    let bot = Bot::new("123456:TESTAPI_DUMMY_TOKEN");
    let link = "https://t.me/test_lifecycle_channel";

    // 1. Create lock
    let lock_id = add_lock(&bot, link, None).await;
    assert!(lock_id > 0, "add_lock must return a valid positive ID");

    // 2. Retrieve lock
    let lock = get_lock(lock_id)
        .await
        .expect("created lock must exist in Redis");
    assert_eq!(lock.id, lock_id);
    assert_eq!(lock.link, link);
    assert_eq!(lock.identifier, "@test_lifecycle_channel");
    assert_eq!(lock.mode, "optional");
    assert!(lock.display_override.is_empty());
    assert_eq!(lock.expires_at, 0);
    assert_eq!(lock.member_cap, 0);
    assert!(lock.reserve_link.is_empty());

    // 3. Update fields
    set_display_name(lock_id, "Test Lifecycle Channel").await;
    assert!(
        set_time_limit(lock_id, "14").await,
        "set_time_limit should succeed for '14'"
    );
    assert!(
        set_member_cap(lock_id, "250").await,
        "set_member_cap should succeed for '250'"
    );
    set_reserve_link(lock_id, "https://t.me/test_lifecycle_backup").await;

    // 4. Verify updated fields
    let updated = get_lock(lock_id)
        .await
        .expect("updated lock must exist in Redis");
    assert_eq!(updated.display_override, "Test Lifecycle Channel");
    assert!(
        updated.expires_at > now_epoch() + 13 * 86400,
        "expires_at must be ~14 days in future"
    );
    assert_eq!(updated.member_cap, 250);
    assert_eq!(updated.reserve_link, "https://t.me/test_lifecycle_backup");

    // 5. List locks
    let all_locks = list_locks().await;
    assert!(
        all_locks.iter().any(|l| l.id == lock_id),
        "created lock must appear in list_locks()"
    );

    // 6. Populate counter & cache keys for this lock to test deep purge
    cache_status(lock_id, 99901, true).await;
    cache_status(lock_id, 99902, false).await;

    // 7. Delete lock
    delete_lock(lock_id).await;

    // 8. Verify all associated Redis keys are purged
    assert!(
        get_lock(lock_id).await.is_none(),
        "lock hash must be deleted"
    );
    assert!(
        !list_locks().await.iter().any(|l| l.id == lock_id),
        "deleted lock must not appear in list_locks()"
    );

    let mut c = conn().await.expect("redis connection must succeed");
    let hash_exists: bool = redis::cmd("EXISTS")
        .arg(lock_hash_key(lock_id))
        .query_async(&mut c)
        .await
        .unwrap_or(false);
    assert!(!hash_exists, "lock hash key must not exist");

    let already_exists: bool = redis::cmd("EXISTS")
        .arg(already_count_key(lock_id))
        .query_async(&mut c)
        .await
        .unwrap_or(false);
    assert!(
        !already_exists,
        "already_count key must be purged on delete"
    );

    let linked_exists: bool = redis::cmd("EXISTS")
        .arg(linked_count_key(lock_id))
        .query_async(&mut c)
        .await
        .unwrap_or(false);
    assert!(!linked_exists, "linked_count key must be purged on delete");

    // Cleanup test user keys
    let _: Result<(), _> = redis::cmd("DEL")
        .arg(joined_key(lock_id, 99901))
        .arg(counted_key(lock_id, 99901))
        .arg(joined_key(lock_id, 99902))
        .arg(counted_key(lock_id, 99902))
        .query_async(&mut c)
        .await;
}

/// 2. Cache Status Lua State Transitions (Risk: HIGH)
/// Verifies state transitions and atomic counters (already, pending, linked, leave).
#[tokio::test]
#[ignore]
async fn test_cache_status_lua_transitions() {
    let test_lock_id = -99981;
    let user_a = 777101;
    let user_b = 777102;

    let mut c = conn().await.expect("redis connection must succeed");

    // Clean initial test state
    let _: Result<(), _> = redis::cmd("DEL")
        .arg(already_count_key(test_lock_id))
        .arg(linked_count_key(test_lock_id))
        .arg(counted_key(test_lock_id, user_a))
        .arg(joined_key(test_lock_id, user_a))
        .arg(counted_key(test_lock_id, user_b))
        .arg(joined_key(test_lock_id, user_b))
        .query_async(&mut c)
        .await;

    // Step 1: User A is already a member when first observed
    cache_status(test_lock_id, user_a, true).await;

    let state_a: Option<String> = redis::cmd("GET")
        .arg(counted_key(test_lock_id, user_a))
        .query_async(&mut c)
        .await
        .unwrap_or(None);
    assert_eq!(state_a.as_deref(), Some("already"));

    let joined_a: Option<String> = redis::cmd("GET")
        .arg(joined_key(test_lock_id, user_a))
        .query_async(&mut c)
        .await
        .unwrap_or(None);
    assert_eq!(joined_a.as_deref(), Some("1"));

    let count_already: i64 = redis::cmd("GET")
        .arg(already_count_key(test_lock_id))
        .query_async(&mut c)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    assert_eq!(count_already, 1, "already_count must be 1");

    let count_linked: i64 = linked_count(test_lock_id).await;
    assert_eq!(count_linked, 0, "linked_count must be 0");

    // Step 2: User B is NOT a member when first observed -> state 'pending'
    cache_status(test_lock_id, user_b, false).await;

    let state_b: Option<String> = redis::cmd("GET")
        .arg(counted_key(test_lock_id, user_b))
        .query_async(&mut c)
        .await
        .unwrap_or(None);
    assert_eq!(state_b.as_deref(), Some("pending"));

    let count_already_after_b: i64 = redis::cmd("GET")
        .arg(already_count_key(test_lock_id))
        .query_async(&mut c)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    assert_eq!(count_already_after_b, 1, "already_count must remain 1");

    // Step 3: User B joins later via link -> state becomes 'linked', linked_count +1
    cache_status(test_lock_id, user_b, true).await;

    let state_b_linked: Option<String> = redis::cmd("GET")
        .arg(counted_key(test_lock_id, user_b))
        .query_async(&mut c)
        .await
        .unwrap_or(None);
    assert_eq!(state_b_linked.as_deref(), Some("linked"));

    let count_linked_after_join: i64 = linked_count(test_lock_id).await;
    assert_eq!(
        count_linked_after_join, 1,
        "linked_count must increment to 1"
    );

    // Step 4: User B leaves channel -> state returns to 'pending', linked_count -1
    cache_status(test_lock_id, user_b, false).await;

    let state_b_left: Option<String> = redis::cmd("GET")
        .arg(counted_key(test_lock_id, user_b))
        .query_async(&mut c)
        .await
        .unwrap_or(None);
    assert_eq!(state_b_left.as_deref(), Some("pending"));

    let count_linked_after_leave: i64 = linked_count(test_lock_id).await;
    assert_eq!(
        count_linked_after_leave, 0,
        "linked_count must decrement to 0"
    );

    // Clean up test keys
    let _: Result<(), _> = redis::cmd("DEL")
        .arg(already_count_key(test_lock_id))
        .arg(linked_count_key(test_lock_id))
        .arg(counted_key(test_lock_id, user_a))
        .arg(joined_key(test_lock_id, user_a))
        .arg(counted_key(test_lock_id, user_b))
        .arg(joined_key(test_lock_id, user_b))
        .query_async(&mut c)
        .await;
}

/// 3. Mandatory Locks Filtering (Risk: CRITICAL)
/// Sets up 5 locks simultaneously:
/// - Lock 1: mandatory, active, no expiry -> INCLUDED
/// - Lock 2: optional -> EXCLUDED
/// - Lock 3: mandatory but expired -> EXCLUDED
/// - Lock 4: mandatory but at capacity (linked_count >= member_cap) -> EXCLUDED
/// - Lock 5: mandatory but missing a valid chat ID -> EXCLUDED
#[tokio::test]
#[ignore]
async fn test_mandatory_locks_filtering() {
    let bot = Bot::new("123456:TESTAPI_DUMMY_TOKEN");

    // Create 5 locks
    let id1 = add_lock(&bot, "https://t.me/test_mand_active", None).await;
    let id2 = add_lock(&bot, "https://t.me/test_mand_optional", None).await;
    let id3 = add_lock(&bot, "https://t.me/test_mand_expired", None).await;
    let id4 = add_lock(&bot, "https://t.me/test_mand_capped", None).await;
    let id5 = add_lock(&bot, "https://instagram.com/not_telegram", None).await;

    let created_ids = vec![id1, id2, id3, id4, id5];
    assert!(created_ids.iter().all(|&id| id > 0));

    let mut c = conn().await.expect("redis connection must succeed");

    // Lock 1: Mandatory, active, unexpired
    set_field(id1, "mode", "mandatory").await;

    // Lock 2: Optional
    set_field(id2, "mode", "optional").await;

    // Lock 3: Mandatory but expired (1 hour ago)
    set_field(id3, "mode", "mandatory").await;
    set_field(id3, "expires_at", &(now_epoch() - 3600).to_string()).await;

    // Lock 4: Mandatory but capped (member_cap = 5, linked_count = 5)
    set_field(id4, "mode", "mandatory").await;
    set_field(id4, "member_cap", "5").await;
    let _: Result<(), _> = redis::cmd("SET")
        .arg(linked_count_key(id4))
        .arg("5")
        .query_async(&mut c)
        .await;

    // Lock 5: Mandatory but no chat ID (instagram link, identifier is empty)
    set_field(id5, "mode", "mandatory").await;
    set_field(id5, "identifier", "").await;

    // Query mandatory locks
    let mand = mandatory_locks().await;

    // Clean up locks before assertions to guarantee no test leaks
    for &id in &created_ids {
        delete_lock(id).await;
    }

    // Assertions: Lock 1 must be present, Locks 2-5 must be absent
    assert!(
        mand.iter().any(|l| l.id == id1),
        "Lock 1 (active mandatory) must be returned by mandatory_locks()"
    );
    assert!(
        !mand.iter().any(|l| l.id == id2),
        "Lock 2 (optional) must be excluded"
    );
    assert!(
        !mand.iter().any(|l| l.id == id3),
        "Lock 3 (expired) must be excluded"
    );
    assert!(
        !mand.iter().any(|l| l.id == id4),
        "Lock 4 (member cap reached) must be excluded"
    );
    assert!(
        !mand.iter().any(|l| l.id == id5),
        "Lock 5 (missing chat ID) must be excluded"
    );
}

/// 4. Concurrency Safety: Atomic Lua counter updates with 20 parallel tasks (Risk: HIGH)
#[tokio::test]
#[ignore]
async fn test_cache_status_concurrent_calls_no_lost_updates() {
    let test_lock_id = -99992;
    let mut c = conn().await.expect("redis connection must succeed");

    // Clean initial test state
    let _: Result<(), _> = redis::cmd("DEL")
        .arg(already_count_key(test_lock_id))
        .arg(linked_count_key(test_lock_id))
        .query_async::<()>(&mut c)
        .await;

    for i in 0..20 {
        let uid = 888000 + i;
        let _: Result<(), _> = redis::cmd("DEL")
            .arg(counted_key(test_lock_id, uid))
            .arg(joined_key(test_lock_id, uid))
            .query_async::<()>(&mut c)
            .await;
    }

    // 1. Concurrently execute 20 "already member" calls
    let mut handles = Vec::new();
    for i in 0..20 {
        let uid = 888000 + i;
        handles.push(tokio::spawn(async move {
            cache_status(test_lock_id, uid, true).await;
        }));
    }
    for h in handles {
        h.await.expect("task must join");
    }

    let already_total: i64 = redis::cmd("GET")
        .arg(already_count_key(test_lock_id))
        .query_async(&mut c)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    assert_eq!(
        already_total, 20,
        "already_count must be exactly 20 after 20 concurrent new-member calls"
    );

    // 2. Concurrently execute 20 "not joined" calls for 20 new users (999000..999020)
    for i in 0..20 {
        let uid = 999000 + i;
        let _: Result<(), _> = redis::cmd("DEL")
            .arg(counted_key(test_lock_id, uid))
            .arg(joined_key(test_lock_id, uid))
            .query_async::<()>(&mut c)
            .await;
    }

    let mut handles_pending = Vec::new();
    for i in 0..20 {
        let uid = 999000 + i;
        handles_pending.push(tokio::spawn(async move {
            cache_status(test_lock_id, uid, false).await;
        }));
    }
    for h in handles_pending {
        h.await.expect("task must join");
    }

    // 3. Concurrently transition all 20 users from pending -> linked
    let mut handles_join = Vec::new();
    for i in 0..20 {
        let uid = 999000 + i;
        handles_join.push(tokio::spawn(async move {
            cache_status(test_lock_id, uid, true).await;
        }));
    }
    for h in handles_join {
        h.await.expect("task must join");
    }

    let linked_total: i64 = linked_count(test_lock_id).await;
    assert_eq!(
        linked_total, 20,
        "linked_count must be exactly 20 after 20 concurrent pending->linked transitions"
    );

    // 4. Concurrently transition 10 of those users from linked -> left (pending)
    let mut handles_leave = Vec::new();
    for i in 0..10 {
        let uid = 999000 + i;
        handles_leave.push(tokio::spawn(async move {
            cache_status(test_lock_id, uid, false).await;
        }));
    }
    for h in handles_leave {
        h.await.expect("task must join");
    }

    let linked_after_leave: i64 = linked_count(test_lock_id).await;
    assert_eq!(
        linked_after_leave, 10,
        "linked_count must be exactly 10 after 10 concurrent linked->left transitions"
    );

    // Cleanup all keys
    let _: Result<(), _> = redis::cmd("DEL")
        .arg(already_count_key(test_lock_id))
        .arg(linked_count_key(test_lock_id))
        .query_async::<()>(&mut c)
        .await;

    for i in 0..20 {
        let uid1 = 888000 + i;
        let uid2 = 999000 + i;
        let _: Result<(), _> = redis::cmd("DEL")
            .arg(counted_key(test_lock_id, uid1))
            .arg(joined_key(test_lock_id, uid1))
            .arg(counted_key(test_lock_id, uid2))
            .arg(joined_key(test_lock_id, uid2))
            .query_async::<()>(&mut c)
            .await;
    }
}
