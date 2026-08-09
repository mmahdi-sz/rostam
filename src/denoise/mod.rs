//! Audio denoising via DeepFilterNet, in-process through `crate::stt::deepfilter`.
//!
//! (Previously a sidecar service on port 8765; no longer used.)
//!
//! Users send voice or audio files; the handler converts, denoises,
//! and returns the cleaned audio. Accurate model is paywalled.

mod handle;
pub use handle::{enter_denoise, handle_denoise_audio, handle_denoise_cancel};
