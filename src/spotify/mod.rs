//! Spotify single-track downloader module.

pub mod cancel;
pub mod client;
pub mod extract;
pub mod handle;
pub mod search;
pub mod tagging;

#[allow(unused_imports)]
pub use cancel::{cancel_spotify_job, register_spotify_cancel, unregister_spotify_cancel};
pub use extract::extract_spotify_track_id;
pub use handle::handle_spotify_url;
