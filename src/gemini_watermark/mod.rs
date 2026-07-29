//! Gemini AI watermark detection and Moebius ONNX inpainting removal subsystem.

// `remove` (the old gwt-mini-binary multi-pass remover) is no longer wired
// in — replaced by the Moebius ONNX pipeline (`crate::moebius`) called from
// `handle.rs`. Left on disk, undeclared, as a rollback reference.
pub mod handle;
pub use handle::{CB_GWM_CANCEL, enter_gwm, handle_gwm_cancel, handle_gwm_image};
