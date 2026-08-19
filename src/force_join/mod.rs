//! Multiple mandatory/optional membership locks stored in Redis (Hash).
//!
//! Each lock: numeric id, link, chat identifier (`@username` or numeric ID — for membership check via
//! `getChatMember`), mode (`mandatory`/`optional`). Only mandatory identifiable locks
//! (valid chat identifier) are checked; optional ones display their link without membership verification.

pub mod cache;
pub mod check;
pub mod conn;
pub mod db;
pub mod jalali;
pub mod types;
pub mod ui;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use cache::{cache_status, on_chat_member_update};
#[allow(unused_imports)]
pub use check::{
    bot_has_full_access, check_lock_membership, is_joined, is_joined_live, toggle_lock_mode,
};
#[allow(unused_imports)]
pub use conn::{already_count_key, counted_key, joined_key, linked_count_key, lock_hash_key};
#[allow(unused_imports)]
pub use db::{
    add_lock, delete_lock, get_lock, is_enabled, list_locks, mandatory_locks, set_display_name,
    set_enabled, set_field, set_member_cap, set_reserve_link, set_time_limit, toggle_enabled,
};
#[allow(unused_imports)]
pub use types::*;
#[allow(unused_imports)]
pub use ui::*;
