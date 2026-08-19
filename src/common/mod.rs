//! Shared utilities and generic infrastructure.

pub mod cpu_broker;
pub mod dir;
pub mod ffmpeg;
pub mod format;
pub mod job;
pub mod keyboard;
pub mod ticker;

pub use cpu_broker::CpuBrokerGuard;
pub use dir::TempDirGuard;
pub use job::{JobGuard, JobRegistry};
pub use keyboard::job_cancel_keyboard;
pub use ticker::{ProgressTicker, ProgressTickerHandle};
