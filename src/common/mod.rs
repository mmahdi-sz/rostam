//! Shared utilities and generic infrastructure.

pub mod cpu_broker;
pub mod dir;
pub mod ffmpeg;
pub mod format;
pub mod job;
pub mod keyboard;
pub mod link;
pub mod ticker;

pub use cpu_broker::CpuBrokerGuard;
pub use dir::TempDirGuard;
pub use job::{JobGuard, JobRegistry};
pub use keyboard::job_cancel_keyboard;
pub use link::{
    extract_entity_links, extract_message_links, extract_plain_urls, extract_surge_url, slice_utf16,
};
pub use ticker::{ProgressTicker, ProgressTickerHandle};
