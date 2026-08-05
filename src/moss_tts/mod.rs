//! MOSS-TTS-Nano (100M) Text-to-Speech and Voice Cloning module.

pub mod engine;
pub mod handle;

pub use handle::{
    enter_tts, handle_tts_cancel, handle_tts_mode_clone, handle_tts_mode_default, handle_tts_text,
    handle_tts_voice_sample,
};
