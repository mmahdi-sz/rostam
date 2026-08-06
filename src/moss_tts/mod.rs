//! Persian TTS via Piper (fa_IR Piper model) with HomoFast eSpeak G2P frontend; English TTS via edge-tts (en-US-AvaNeural).

pub mod engine;
pub mod handle;
pub mod homofast;

pub use handle::{enter_tts, handle_tts_cancel, handle_tts_text};
