//! File compression subsystem supporting ZIP, 7Z, and RAR archives.

mod config;
mod engine;
pub mod handle;

pub use config::CompressConfig;
pub use handle::{
    CB_FC_PREFIX, CB_TOOLS_FILECOMPRESS, enter_filecompress, handle_fc_callback,
    handle_fc_done_text, handle_fc_file, handle_fc_password_text,
};
