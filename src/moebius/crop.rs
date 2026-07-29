//! Windowing pipeline: given the located Gemini "sparkle" watermark, crop a
//! model-sized (512×512) window around it, build the inpaint mask, and later
//! paste the result back.
//!
//! The model is exported at a fixed 512×512 resolution (its cross-attention
//! uses a rel-position embedding tied to that training resolution — see
//! `pipeline.rs` docs), so full-resolution images can never be fed to it
//! directly. The watermark's exact position depends on the image's aspect
//! ratio (Gemini 3.5 moved it), so `detect::detect_watermark` finds it
//! dynamically; this module then centers a 512 window on that point.
//!
//! When detection fails but the image is large in both dimensions (≥1024),
//! `fixed_*` reproduce the legacy fixed-corner heuristic as a fallback
//! (Gemini's classic 48px/32px-margin profile, reliable for that size class).
//! For anything else, the pipeline reports "no watermark found" rather than
//! inpainting a guessed region and risking damage to a clean image.
//!
//! For images smaller than 512 in either dimension, the window extends past
//! the image edge; those out-of-bounds samples are edge-clamped (replicated),
//! and `paste_into` discards whatever was written into the clamped region so
//! it never leaks into the output.

use super::detect::Detection;

pub const MODEL_SIZE: u32 = 512;

/// Top-left corner (in original-image coordinates) of the 512×512 crop
/// window. May be negative when the source image is smaller than 512 in that
/// dimension — callers must edge-clamp when sampling.
#[derive(Debug, Clone, Copy)]
pub struct CropWindow {
    pub x0: i64,
    pub y0: i64,
}

/// Watermark bounding box in *local* crop coordinates (0..512), already
/// expanded by a small margin so the inpaint mask fully covers the sparkle
/// icon's antialiased edges rather than clipping them.
#[derive(Debug, Clone, Copy)]
pub struct WatermarkBBox {
    pub x1: u32,
    pub y1: u32,
    pub x2: u32,
    pub y2: u32,
}

/// Extra margin (px) added around the located sparkle so the mask covers its
/// glow/antialiasing. Scales a little with the sparkle size.
const MASK_PAD_MIN: i64 = 12;

/// Center a 512×512 window on `(cx, cy)`, clamped so it never needlessly
/// extends past the bottom-right corner (where it would sample edge-padding
/// instead of real image), and never before (0,0) unless the source is
/// smaller than 512 in that axis (then it falls back to the bottom-right
/// anchor, i.e. the whole image with padding on the top/left).
pub fn window_around(cx: i64, cy: i64, width: u32, height: u32) -> CropWindow {
    let half = MODEL_SIZE as i64 / 2;
    let axis = |c: i64, dim: u32| -> i64 {
        let max0 = dim as i64 - MODEL_SIZE as i64; // negative if dim < 512
        if max0 <= 0 {
            max0
        } else {
            (c - half).clamp(0, max0)
        }
    };
    CropWindow {
        x0: axis(cx, width),
        y0: axis(cy, height),
    }
}

/// Mask bbox (local 0..512 coords) for a dynamically-detected watermark.
pub fn bbox_from_detection(det: &Detection, window: CropWindow) -> WatermarkBBox {
    let m = MODEL_SIZE as i64;
    let pad = MASK_PAD_MIN.max(det.size / 6);
    let half = det.size / 2 + pad;
    let lcx = det.cx - window.x0;
    let lcy = det.cy - window.y0;
    let x1 = (lcx - half).clamp(0, m);
    let x2 = (lcx + half).clamp(0, m);
    let y1 = (lcy - half).clamp(0, m);
    let y2 = (lcy + half).clamp(0, m);
    WatermarkBBox {
        x1: x1 as u32,
        y1: y1 as u32,
        x2: x2 as u32,
        y2: y2 as u32,
    }
}

/// Legacy fixed bottom-right crop window (used only in the ≥1024 fallback).
pub fn fixed_crop_window(width: u32, height: u32) -> CropWindow {
    CropWindow {
        x0: width as i64 - MODEL_SIZE as i64,
        y0: height as i64 - MODEL_SIZE as i64,
    }
}

/// Legacy fixed watermark bbox (local coords) for the ≥1024 fallback: Gemini's
/// classic 48px sparkle, 32px margin from the bottom-right edges.
pub fn fixed_bbox_local() -> WatermarkBBox {
    let (wm_size, margin): (i64, i64) = (48, 32);
    let m = MODEL_SIZE as i64;
    let x2 = (m - margin + MASK_PAD_MIN).clamp(0, m);
    let x1 = (m - margin - wm_size - MASK_PAD_MIN).clamp(0, m);
    let y2 = (m - margin + MASK_PAD_MIN).clamp(0, m);
    let y1 = (m - margin - wm_size - MASK_PAD_MIN).clamp(0, m);
    WatermarkBBox {
        x1: x1 as u32,
        y1: y1 as u32,
        x2: x2 as u32,
        y2: y2 as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_centers_on_detection_when_room() {
        // watermark well inside a large image -> window centered on it
        let w = window_around(1000, 1050, 2000, 2000);
        assert_eq!(w.x0, 1000 - 256);
        assert_eq!(w.y0, 1050 - 256);
    }

    #[test]
    fn window_clamps_at_bottom_right_corner() {
        // watermark near the corner -> window pinned to the max origin, not past it
        let w = window_around(1180, 1180, 1200, 1200);
        assert_eq!(w.x0, 1200 - 512);
        assert_eq!(w.y0, 1200 - 512);
    }

    #[test]
    fn window_smaller_than_512_uses_full_image_anchor() {
        let w = window_around(150, 200, 300, 400);
        assert_eq!(w.x0, 300 - 512);
        assert_eq!(w.y0, 400 - 512);
        assert!(w.x0 < 0 && w.y0 < 0);
    }

    #[test]
    fn detection_bbox_surrounds_local_center() {
        let det = Detection {
            cx: 1000,
            cy: 1050,
            size: 60,
            score: 0.9,
        };
        let win = window_around(det.cx, det.cy, 2000, 2000);
        let b = bbox_from_detection(&det, win);
        // local center should be ~256,256 -> bbox straddles it
        assert!(b.x1 < 256 && b.x2 > 256);
        assert!(b.y1 < 256 && b.y2 > 256);
        assert!(b.x2 <= MODEL_SIZE && b.y2 <= MODEL_SIZE);
    }

    #[test]
    fn fixed_bbox_stays_in_bottom_right_quadrant() {
        let b = fixed_bbox_local();
        assert!(b.x1 > MODEL_SIZE / 2);
        assert!(b.y1 > MODEL_SIZE / 2);
        assert!(b.x2 <= MODEL_SIZE && b.y2 <= MODEL_SIZE);
    }
}
