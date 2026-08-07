//! Audio denoising via DeepFilterNet, in-process through `crate::stt::deepfilter`.
//!
//! (قبلاً یک sidecar روی پورت 8765 بود؛ آن سرویس دیگر استفاده نمی‌شود.)
//!
//! Users send voice or audio files; the handler converts, denoises,
//! and returns the cleaned audio. Accurate model is paywalled.

mod handle;
pub use handle::{enter_denoise, handle_denoise_audio, handle_denoise_cancel};
