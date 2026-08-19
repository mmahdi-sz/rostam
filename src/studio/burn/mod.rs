//! Hardsub subtitle burning module for Photo & Video Magic Studio (`studio_burn`).
//!
//! Handles subtitle detection (SRT, ASS/SSA, WebVTT), ASS native style preservation vs SRT/VTT
//! force_style, background video ingestion with a live download ticker, brokered ffmpeg execution
//! with `-progress pipe:1`, cancel-aware waiting, and re-arming back to the burn prompt.
//!
//! Inputs are copied to fixed in-work-dir names (`input.<ext>`, `sub.<ext>`) so neither the
//! filesystem path nor the ffmpeg filtergraph ever carries a user-controlled string.

pub mod handle;
pub mod runner;
pub mod session;
pub mod subtitle;

#[allow(unused_imports)]
pub use handle::*;
#[allow(unused_imports)]
pub use runner::*;
#[allow(unused_imports)]
pub use session::*;
#[allow(unused_imports)]
pub use subtitle::*;

/// Hardsub re-encode is CPU-bound; anything longer would hold a broker slot for hours.
pub const MAX_BURN_DURATION_SECS: u64 = 7200;
/// Telegram Bot API upload ceiling.
pub const MAX_UPLOAD_BYTES: u64 = 2000 * 1024 * 1024;
