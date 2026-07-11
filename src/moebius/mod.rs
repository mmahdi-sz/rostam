//! Moebius ONNX inpainting pipeline — replaces the old `gwt-mini`-binary
//! Gemini watermark remover. Crops a 512×512 window anchored to the image's
//! bottom-right corner, masks the watermark's bounding box, runs DDIM
//! diffusion (VAE encode -> UNet x N steps w/ CFG -> VAE decode) through the
//! ONNX Runtime `ort` crate in-process, feather-blends the result, and pastes
//! it back into the full-resolution image. See `pipeline.rs` for the full
//! step-by-step doc comment.

mod cpu;
mod crop;
mod detect;
mod imaging;
mod model;
mod pipeline;
mod scheduler;

pub use pipeline::{remove_watermark, MoebiusError};
