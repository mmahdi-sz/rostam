//! Shared utilities and generic infrastructure.

pub mod format;
pub mod job;

pub use job::{JobGuard, JobRegistry};
