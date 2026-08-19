//! Fast parallel direct-link downloader subsystem using Surge DM daemon.

pub mod client;
pub mod engine;
pub mod handle;
pub mod probe;
pub mod types;
pub mod ui;

#[allow(unused_imports)]
pub use probe::available_disk_space;
pub use probe::{detect_social_platform, is_direct_link};
pub use types::{
    CB_SURGE_CANCEL, CB_SURGE_CONFIRM_ORIGINAL, CB_SURGE_CONFIRM_RENAME, CB_TOOLS_SURGE,
};
pub use handle::{
    enter_surge_dl, handle_surge_cancel, handle_surge_confirm_original,
    handle_surge_confirm_rename, handle_surge_rename_text, handle_surge_text,
};
