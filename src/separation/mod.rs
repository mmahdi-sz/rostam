//! Audio/video vocal and instrumental separation subsystem.

mod client;
mod error;
pub mod handle;
mod types;

pub use handle::{
    CB_SEP_PREFIX, enter_separation, handle_separation_audio, handle_separation_callback,
};
