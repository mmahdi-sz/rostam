//! Fast parallel direct-link downloader subsystem using Surge DM daemon.

mod handle;
pub use handle::{
    CB_SURGE_CANCEL, CB_SURGE_CONFIRM_ORIGINAL, CB_SURGE_CONFIRM_RENAME, CB_TOOLS_SURGE,
    enter_surge_dl, handle_surge_cancel, handle_surge_confirm_original,
    handle_surge_confirm_rename, handle_surge_rename_text, handle_surge_text, is_direct_link,
};
#[allow(unused_imports)]
pub use handle::available_disk_space;
