//! Redeem code system for time-limited rank upgrades.
//!
//! Admins generate one-time codes with `/se gen <days> <rank> <uses>`.
//! Users redeem via the rank panel. See [`generate`] for code creation,
//! [`handle`] for Telegram callback handling.

pub mod generate;
pub mod handle;
pub mod panel;
pub mod panel_state;
pub mod store;
