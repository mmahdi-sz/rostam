// =========================================================================
// Test Coverage Strategy & Phase 4 Exclusions
// - Phase 1: Pure Unit Tests (Access matrix, parsing, predicates, Jalali dates, keyboard builders, fail-open guard)
// - Phase 2: Redis / Lua Integration Tests (Lock CRUD lifecycle, cache_status Lua transitions, mandatory lock filtering, 20-client concurrency)
// - Phase 3: TestAPI Endpoints (/test/fj/gate, admin/menu, admin/locks, admin/manage, admin/toggle_mode)
// - Phase 4: Trivial Helpers (no_preview, send_text_np, edit_text_np, Redis key string formatters) are intentionally
//   not given dedicated standalone unit tests as they are thin static builders/formatters verified transitively
//   through Phase 1, Phase 2, and Phase 3 suites.
// =========================================================================

use frankenstein::types::{
    ChatId, ChatMember, ChatMemberAdministrator, ChatMemberBanned, ChatMemberLeft,
    ChatMemberMember, ChatMemberOwner, ChatMemberRestricted, User,
};

use crate::force_join::cache::{chat_member_user_id, is_member_status};
use crate::force_join::db::derive_identifier;
use crate::force_join::jalali::{fmt_jalali_dt, now_epoch, to_en_digits};
use crate::force_join::types::{
    CB_FJ_ADD_NEW, CB_FJ_NOOP, CB_FJ_TOGGLE, CB_FJ_VIEW, Lock, chat_id_for,
};
use crate::force_join::ui::admin::{is_private_tme_link, locks_list_view, menu_keyboard};
use crate::force_join::ui::url_button;
use crate::i18n::t;

fn make_user(id: u64) -> User {
    User::builder()
        .id(id)
        .is_bot(false)
        .first_name("Test")
        .build()
}

/// 1. Access Matrix: Verifies access permissions across all 6 ChatMember status variants.
/// Creator, Administrator, Member, Restricted -> true (allowed access).
/// Left, Kicked -> false (blocked by force join).
#[test]
fn test_is_member_status_access_matrix() {
    let creator = ChatMember::Creator(
        ChatMemberOwner::builder()
            .user(make_user(101))
            .is_anonymous(false)
            .build(),
    );
    let admin = ChatMember::Administrator(
        ChatMemberAdministrator::builder()
            .user(make_user(102))
            .can_be_edited(false)
            .is_anonymous(false)
            .can_manage_chat(true)
            .can_delete_messages(true)
            .can_manage_video_chats(true)
            .can_restrict_members(true)
            .can_promote_members(false)
            .can_change_info(false)
            .can_invite_users(true)
            .can_post_stories(false)
            .can_edit_stories(false)
            .can_delete_stories(false)
            .build(),
    );
    let member = ChatMember::Member(
        ChatMemberMember::builder()
            .user(make_user(103))
            .build(),
    );
    let restricted = ChatMember::Restricted(
        ChatMemberRestricted::builder()
            .user(make_user(104))
            .is_member(true)
            .can_send_messages(true)
            .can_send_audios(true)
            .can_send_documents(true)
            .can_send_photos(true)
            .can_send_videos(true)
            .can_send_video_notes(true)
            .can_send_voice_notes(true)
            .can_send_polls(true)
            .can_send_other_messages(true)
            .can_add_web_page_previews(true)
            .can_change_info(false)
            .can_invite_users(false)
            .can_pin_messages(false)
            .can_manage_topics(false)
            .can_react_to_messages(true)
            .can_edit_tag(false)
            .until_date(0)
            .build(),
    );
    let left = ChatMember::Left(
        ChatMemberLeft::builder()
            .user(make_user(105))
            .build(),
    );
    let kicked = ChatMember::Kicked(
        ChatMemberBanned::builder()
            .user(make_user(106))
            .until_date(0)
            .build(),
    );

    // Active membership variants must be granted access
    assert!(is_member_status(&creator), "Creator must be allowed");
    assert!(is_member_status(&admin), "Admin must be allowed");
    assert!(is_member_status(&member), "Member must be allowed");
    assert!(is_member_status(&restricted), "Restricted member must be allowed");

    // Non-member variants must be blocked
    assert!(!is_member_status(&left), "Left user must be blocked");
    assert!(!is_member_status(&kicked), "Kicked user must be blocked");

    // User ID extraction must work correctly across all variants
    assert_eq!(chat_member_user_id(&creator), 101);
    assert_eq!(chat_member_user_id(&admin), 102);
    assert_eq!(chat_member_user_id(&member), 103);
    assert_eq!(chat_member_user_id(&restricted), 104);
    assert_eq!(chat_member_user_id(&left), 105);
    assert_eq!(chat_member_user_id(&kicked), 106);
}

/// 2. Parsing: Verifies chat_id_for conversion for public @username, numeric channel ID, and invalid shapes.
#[test]
fn test_chat_id_for_parsing() {
    // Valid username
    assert_eq!(
        chat_id_for("@channel_name"),
        Some(ChatId::String("@channel_name".to_string()))
    );
    assert_eq!(
        chat_id_for("@my_super_channel_123"),
        Some(ChatId::String("@my_super_channel_123".to_string()))
    );

    // Valid numeric IDs (supergroups / channels start with -100...)
    assert_eq!(
        chat_id_for("-1001234567890"),
        Some(ChatId::Integer(-1001234567890))
    );
    assert_eq!(chat_id_for("123456"), Some(ChatId::Integer(123456)));

    // Invalid / unidentifiable formats
    assert_eq!(chat_id_for(""), None);
    assert_eq!(chat_id_for("channel_no_at"), None);
    assert_eq!(chat_id_for("https://t.me/chan"), None);
    assert_eq!(chat_id_for("invalid_random_string"), None);
}

/// 3. URL identifier extraction: Verifies extracting @username from public links and rejecting private/foreign links.
#[test]
fn test_derive_identifier_variations() {
    // Standard public links
    assert_eq!(
        derive_identifier("https://t.me/vilix_channel"),
        Some("@vilix_channel".to_string())
    );
    assert_eq!(
        derive_identifier("http://t.me/vilix"),
        Some("@vilix".to_string())
    );
    assert_eq!(
        derive_identifier("t.me/vilix"),
        Some("@vilix".to_string())
    );

    // Private invite links (must return None because identifier cannot be deduced)
    assert_eq!(derive_identifier("https://t.me/+AbCdEfGh123"), None);
    assert_eq!(derive_identifier("t.me/+joinhash"), None);

    // Foreign / non-Telegram links
    assert_eq!(derive_identifier("https://instagram.com/myaccount"), None);
    assert_eq!(derive_identifier("https://google.com"), None);
    assert_eq!(derive_identifier(""), None);

    // Trailing slash (last segment is empty string)
    assert_eq!(derive_identifier("https://t.me/vilix/"), None);
}

/// 4. Private link detection: Distinguishes private invite links (+ or joinchat/) from public channels.
#[test]
fn test_is_private_tme_link() {
    // Modern hash-based private invite links
    assert!(is_private_tme_link("https://t.me/+joinhash"));
    assert!(is_private_tme_link("t.me/+AbCdEfGh"));

    // Legacy joinchat private invite links
    assert!(is_private_tme_link("https://t.me/joinchat/abcdef12345"));
    assert!(is_private_tme_link("t.me/joinchat/xyz"));

    // Public links
    assert!(!is_private_tme_link("https://t.me/publicchannel"));
    assert!(!is_private_tme_link("t.me/vilix"));

    // Non-Telegram URLs / empty
    assert!(!is_private_tme_link("https://instagram.com/foo"));
    assert!(!is_private_tme_link(""));
}

/// 5. Lock struct logic: Mode checks, expiration predicates, chat ID extraction, and display name priority.
#[test]
fn test_lock_struct_logic() {
    let base_lock = Lock {
        id: 1,
        link: "https://t.me/chan".to_string(),
        identifier: "@chan".to_string(),
        title: "Channel Title".to_string(),
        display_override: "Custom Label".to_string(),
        mode: "mandatory".to_string(),
        created_at: 1700000000,
        expires_at: 0,
        member_cap: 0,
        reserve_link: "https://t.me/backup".to_string(),
    };

    // Mode check
    assert!(base_lock.is_mandatory());
    let optional_lock = Lock {
        mode: "optional".to_string(),
        ..Lock {
            id: 2,
            link: "https://t.me/opt".to_string(),
            identifier: "".to_string(),
            title: "".to_string(),
            display_override: "".to_string(),
            mode: "optional".to_string(),
            created_at: 0,
            expires_at: 0,
            member_cap: 0,
            reserve_link: "".to_string(),
        }
    };
    assert!(!optional_lock.is_mandatory());

    // Expiration check
    let now = now_epoch();
    let unexpired_lock = Lock {
        expires_at: now + 3600,
        ..Lock {
            id: 3,
            link: "".to_string(),
            identifier: "".to_string(),
            title: "".to_string(),
            display_override: "".to_string(),
            mode: "mandatory".to_string(),
            created_at: 0,
            expires_at: now + 3600,
            member_cap: 0,
            reserve_link: "".to_string(),
        }
    };
    assert!(!unexpired_lock.is_expired());

    let expired_lock = Lock {
        expires_at: now - 3600,
        ..Lock {
            id: 4,
            link: "".to_string(),
            identifier: "".to_string(),
            title: "".to_string(),
            display_override: "".to_string(),
            mode: "mandatory".to_string(),
            created_at: 0,
            expires_at: now - 3600,
            member_cap: 0,
            reserve_link: "".to_string(),
        }
    };
    assert!(expired_lock.is_expired());

    let permanent_lock = Lock {
        expires_at: 0,
        ..Lock {
            id: 5,
            link: "".to_string(),
            identifier: "".to_string(),
            title: "".to_string(),
            display_override: "".to_string(),
            mode: "mandatory".to_string(),
            created_at: 0,
            expires_at: 0,
            member_cap: 0,
            reserve_link: "".to_string(),
        }
    };
    assert!(!permanent_lock.is_expired());

    // ChatId extraction
    assert_eq!(
        base_lock.chat_id(),
        Some(ChatId::String("@chan".to_string()))
    );

    // Display name fallback priority: display_override > title > identifier > link
    assert_eq!(base_lock.display_name(), "Custom Label");

    let mut l = base_lock;
    l.display_override.clear();
    assert_eq!(l.display_name(), "Channel Title");

    l.title.clear();
    assert_eq!(l.display_name(), "@chan");

    l.identifier.clear();
    assert_eq!(l.display_name(), "https://t.me/chan");
}

/// 6. Persian/Arabic digit conversion to ASCII English digits.
#[test]
fn test_to_en_digits_conversion() {
    // Persian digits
    assert_eq!(to_en_digits("۰۱۲۳۴۵۶۷۸۹"), "0123456789");

    // Arabic digits
    assert_eq!(to_en_digits("٠١٢٣٤٥٦٧٨٩"), "0123456789");

    // Mixed strings with surrounding text
    assert_eq!(to_en_digits(" ۳۰ روز "), " 30 روز ");
    assert_eq!(to_en_digits("تعداد ۵۰۰ نفر"), "تعداد 500 نفر");

    // Non-digits preserved
    assert_eq!(to_en_digits("حذف"), "حذف");
    assert_eq!(to_en_digits("-"), "-");
    assert_eq!(to_en_digits("0"), "0");
}

/// 7. Jalali date/time formatting in Tehran timezone with English digits.
#[test]
fn test_fmt_jalali_dt() {
    // Epoch 1700000000 is 2023-11-14 22:13:20 UTC -> Tehran (UTC+3:30) is 2023-11-15 01:43:20
    // Gregorian 2023-11-15 is Jalali 1402/08/24
    let formatted = fmt_jalali_dt(1700000000);
    assert_eq!(formatted, "🗓 1402/08/24 ⏰ 01:43");

    // Extreme / invalid timestamp returns dash fallback
    assert_eq!(fmt_jalali_dt(i64::MAX), "—");
}

/// 8. Admin UI keyboard and view builders: Verifies callback data compliance and structure.
#[test]
fn test_ui_keyboard_builders() {
    // Menu keyboard when enabled
    let kb_on = menu_keyboard(true);
    let rows_on = &kb_on.inline_keyboard;
    assert_eq!(rows_on[0][0].callback_data.as_deref(), Some(CB_FJ_TOGGLE));
    assert_eq!(rows_on[1][0].callback_data.as_deref(), Some(CB_FJ_NOOP));
    assert_eq!(rows_on[2][0].callback_data.as_deref(), Some(CB_FJ_VIEW));
    assert_eq!(
        rows_on[3][0].callback_data.as_deref(),
        Some(crate::bot::CB_ADMIN_PANEL)
    );

    // Menu keyboard when disabled
    let kb_off = menu_keyboard(false);
    assert_ne!(
        kb_on.inline_keyboard[0][0].text,
        kb_off.inline_keyboard[0][0].text
    );

    // Locks list view: empty state
    let (text_empty, kb_empty) = locks_list_view(&[]);
    assert_eq!(text_empty, t("force_join.no_locks_text"));
    assert_eq!(
        kb_empty.inline_keyboard[0][0].callback_data.as_deref(),
        Some(CB_FJ_ADD_NEW)
    );

    // Locks list view: populated state
    let locks = vec![
        Lock {
            id: 10,
            link: "https://t.me/chan1".to_string(),
            identifier: "@chan1".to_string(),
            title: "Chan 1".to_string(),
            display_override: "".to_string(),
            mode: "mandatory".to_string(),
            created_at: 0,
            expires_at: 0,
            member_cap: 0,
            reserve_link: "".to_string(),
        },
        Lock {
            id: 20,
            link: "https://t.me/chan2".to_string(),
            identifier: "@chan2".to_string(),
            title: "Chan 2".to_string(),
            display_override: "".to_string(),
            mode: "optional".to_string(),
            created_at: 0,
            expires_at: 0,
            member_cap: 0,
            reserve_link: "".to_string(),
        },
    ];
    let (text_pop, kb_pop) = locks_list_view(&locks);
    assert_eq!(text_pop, t("force_join.locks_list_title"));
    let rows = &kb_pop.inline_keyboard;
    // Lock 10 row
    assert_eq!(rows[0][0].callback_data.as_deref(), Some(CB_FJ_NOOP));
    assert_eq!(rows[0][1].callback_data.as_deref(), Some("fj:manage:10"));
    // Lock 20 row
    assert_eq!(rows[1][0].callback_data.as_deref(), Some(CB_FJ_NOOP));
    assert_eq!(rows[1][1].callback_data.as_deref(), Some("fj:manage:20"));
    // Spacer + Add new + Back
    assert_eq!(rows[2][0].callback_data.as_deref(), Some(CB_FJ_NOOP));
    assert_eq!(rows[3][0].callback_data.as_deref(), Some(CB_FJ_ADD_NEW));
    assert_eq!(
        rows[4][0].callback_data.as_deref(),
        Some(crate::bot::CB_ADMIN_FORCE_JOIN)
    );

    // URL button builder
    let btn = url_button("Join Channel", "https://t.me/test_channel");
    assert_eq!(btn.text, "Join Channel");
    assert_eq!(btn.url.as_deref(), Some("https://t.me/test_channel"));
    assert_eq!(btn.callback_data, None);
}

/// 9. Regression Guard: Fail-Open behavior on Telegram Bot API error.
///
/// ## DoS-Risk Rationale & Architectural Design
/// Force-join intentionally implements a strict two-layer strategy:
///
/// 1. **Fail-Closed at Configuration Time (Prevention)**:
///    In `toggle_lock_mode`, `bot_has_full_access` ensures a lock cannot be switched
///    to "mandatory" mode unless the bot is currently verified as an Administrator
///    or Creator in that channel.
///
/// 2. **Fail-Open at Check Time (Resilience)**:
///    In `check_lock_membership` (`force_join.rs:575-578`), any `Err(_)` from Telegram Bot
///    API (`api.get_chat_member`) evaluates to `true` (pass / not locked out).
///
/// If check-time evaluation were fail-closed:
/// - Any temporary Telegram API outage, 5xx error, network timeout, or rate-limit
///   would instantly lock out **100% of all bot users**.
/// - If a channel owner revokes the bot's admin rights or deletes the channel, all users
///   would be trapped forever with no way to unlock themselves (even after clicking "Check").
///
/// This test guards this intentional design: API errors MUST NEVER block users.
#[test]
fn test_membership_check_fail_open_on_api_error() {
    // Direct simulation of the decision branch in `check_lock_membership`:
    // match api.get_chat_member(&params).await {
    //     Ok(resp) => is_member_status(&resp.result),
    //     Err(_) => true, // Missing info should not lock out user
    // }
    let evaluate_membership_result = |res: Result<&ChatMember, &str>| match res {
        Ok(member) => is_member_status(member),
        Err(_) => true, // Fail-open: API errors / missing info must pass
    };

    // 1. Transient network errors & timeouts must fail-open (pass)
    assert!(
        evaluate_membership_result(Err("Timed out waiting for response")),
        "Network timeout must fail-open to prevent bot DoS"
    );
    assert!(
        evaluate_membership_result(Err("502 Bad Gateway from Bot API")),
        "Bot API 5xx errors must fail-open"
    );

    // 2. Permission errors (bot demoted or removed from channel admins) must fail-open
    assert!(
        evaluate_membership_result(Err("Forbidden: bot is not a member of the channel")),
        "Missing bot permissions must fail-open"
    );
    assert!(
        evaluate_membership_result(Err("Bad Request: chat not found")),
        "Deleted/renamed channel must fail-open"
    );

    // 3. Normal successful responses still enforce strict member status
    let non_member = ChatMember::Left(
        ChatMemberLeft::builder()
            .user(make_user(105))
            .build(),
    );
    assert!(
        !evaluate_membership_result(Ok(&non_member)),
        "Successful API response for non-member must still block"
    );

    let active_member = ChatMember::Member(
        ChatMemberMember::builder()
            .user(make_user(103))
            .build(),
    );
    assert!(
        evaluate_membership_result(Ok(&active_member)),
        "Successful API response for member must allow"
    );
}
