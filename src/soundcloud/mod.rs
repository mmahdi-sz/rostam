//! SoundCloud single-track downloader subsystem.

mod cancel;
pub mod extract;
pub mod fetch;
pub mod handle;

pub use cancel::cancel_soundcloud_job;
pub use extract::extract_soundcloud_url;
pub use handle::handle_soundcloud_url;
