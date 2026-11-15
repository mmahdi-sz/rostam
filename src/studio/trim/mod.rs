//! Video trim and edit module for Photo & Video Magic Studio (`studio_trim`).
//!
//! Handles ffprobe metadata extraction, multi-range timestamp parsing (with Persian/Arabic-Indic
//! digit normalization and whitespace tolerance), brokered ffmpeg trimming with copy -> encode fallback,
//! live progress ticker, cancellation, and re-arming back to range collection state.

pub mod engine;
pub mod handle;
pub mod probe;
pub mod range;
pub mod runner;

#[allow(unused_imports)]
pub use crate::studio::is_telegram_audio_guaranteed;
#[allow(unused_imports)]
pub use engine::*;
#[allow(unused_imports)]
pub use handle::*;
#[allow(unused_imports)]
pub use probe::*;
#[allow(unused_imports)]
pub use range::*;
#[allow(unused_imports)]
pub use runner::*;
