// `remove` (the old gwt-mini-binary multi-pass remover) is no longer wired
// in — replaced by the Moebius ONNX pipeline (`crate::moebius`) called from
// `handle.rs`. Left on disk, undeclared, as a rollback reference.
pub mod handle;
pub use handle::{enter_gwm, handle_gwm_image, handle_gwm_cancel, CB_GWM_CANCEL};
