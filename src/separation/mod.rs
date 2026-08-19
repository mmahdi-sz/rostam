//! Audio/video vocal and instrumental separation subsystem.

mod client;
mod error;
pub mod format;
pub mod handle;
pub mod keyboards;
pub mod media;
pub mod quota;
pub mod runner;
mod types;
pub mod upload;

pub(crate) fn log_trace(trace_id: u64, event: &str, details: &str) {
    crate::log::emit("sep", trace_id, event, details);
}

pub use handle::{
    CB_SEP_PREFIX, enter_separation, handle_direct_separation, handle_separation_audio,
    handle_separation_callback,
};
