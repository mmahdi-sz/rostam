//! File compression subsystem supporting ZIP, 7Z, and RAR archives.

pub mod config;
pub mod engine;
pub mod handle;
pub mod pipeline;
pub mod progress;
pub mod session;

pub use config::CompressConfig;
#[cfg(feature = "testapi")]
pub use config::CompressFmt;
pub use handle::{CB_FC_PREFIX, CB_TOOLS_FILECOMPRESS, enter_filecompress, handle_fc_callback};
pub use session::{
    handle_fc_done_text, handle_fc_file, handle_fc_password_text, send_options_menu,
    send_password_need_text,
};
