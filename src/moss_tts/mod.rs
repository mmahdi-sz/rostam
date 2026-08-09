//! Persian TTS via Piper (fa_IR Piper model) with HomoFast eSpeak G2P frontend; English TTS via edge-tts (en-US-AvaNeural).

pub mod engine;
pub mod handle;
pub mod homofast;

pub use handle::{
    CB_TTS_JOB_CANCEL, enter_tts, handle_tts_cancel, handle_tts_text, signal_tts_cancel,
};

// Character cap enforced inside handle; exposed externally for testapi only.
#[cfg(feature = "testapi")]
pub use handle::TTS_MAX_CHARS;

#[cfg(feature = "testapi")]
pub use handle::{tts_cancel_keyboard_for_test, tts_job_cancel_keyboard_for_test};
