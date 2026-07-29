//! YouTube downloading subsystem, quality/format selection, cookie pool integration, and subtitle processing.

pub mod download;
mod extract;
mod fetch;
mod format;
mod handle;
pub mod jalali;
mod lang_names;
mod quality_keyboard;
mod selection;
pub mod trace;
pub mod translator;
mod types;

pub use extract::extract_youtube_urls;
pub use format::escape_markdown_v2;
pub use handle::handle_youtube_url;
pub use quality_keyboard::handle_quality_callback;
// ponytail: log_trace/next_trace_id no longer re-exported — callers use crate::log directly.
