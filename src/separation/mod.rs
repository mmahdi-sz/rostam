mod error;
mod types;
mod client;
pub mod handle;

pub use error::SeparationError;
pub use types::{SeparationMode, SeparationResult};
pub use client::separate_audio;
pub use handle::{
    enter_separation, handle_separation_audio, handle_separation_callback,
    CB_SEP_PREFIX, CB_AI_SEP,
};
