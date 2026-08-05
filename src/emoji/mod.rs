//! Custom emoji pack management system.
//!
//! Allows admins to define named packs of Telegram custom emojis with aliases.
//! Emojis are resolved at render time via [`cache`] and applied with
//! [`i18n::apply_premium_to_md`] or [`i18n::apply_premium_to_html`].

pub mod cache;
pub mod flow;
pub mod handler;
pub mod import;
pub mod panel;
pub mod smart_name;
pub mod store;

pub use flow::{BroadcastMode, FlowManager, FlowState, PendingEmoji};
