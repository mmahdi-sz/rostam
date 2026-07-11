//! Dynamic watermark localization via multi-scale ZNCC template matching.
//!
//! Gemini's sparkle (✦) is always in the bottom-right, but with Gemini 3.5 its
//! exact offset moved and now depends on the image's aspect ratio (see the
//! `notes` in `crop.rs`), so a fixed corner formula no longer hits it. Instead
//! we synthesize a 4-pointed-star template and slide it (at several scales)
//! over the bottom-right region, scoring each position with zero-mean
//! normalized cross-correlation. `abs(zncc)` is used so a *bright* sparkle on a
//! dark background and a *dark* sparkle on a light background both score high.
//!
//! Validated in a Python prototype against a real 768×1376 Gemini image: the
//! best match landed on the star center to within 1px (score 0.866), while a
//! flat gradient scored 0.000 and random noise 0.168 — so the MIN_SCORE gate
//! cleanly separates "found" from "no watermark here".
//!
//! To stay resolution-independent (a 4K image's search region would otherwise
//! be enormous), the region is first downscaled so its longer side is at most
//! `SEARCH_MAX`; the match is found in that space and mapped back to full-res.

use image::{imageops, RgbImage};

/// A located watermark, in full-resolution image coordinates.
#[derive(Debug, Clone, Copy)]
pub struct Detection {
    pub cx: i64,
    pub cy: i64,
    /// Side length of the (square) sparkle box in full-res pixels.
    pub size: i64,
    pub score: f32,
}

/// Longest side the search region is downscaled to before matching.
const SEARCH_MAX: u32 = 480;
/// Confidence gate. 0.55 sits well below the ~0.87 a real sparkle scores and
/// well above the ~0.17 that textured non-watermark content produces.
const MIN_SCORE: f32 = 0.55;
/// Template sizes swept in *downscaled* space. After downscaling the region to
/// SEARCH_MAX, the sparkle lands in roughly this pixel range regardless of the
/// source resolution (small 512px images: ~48px; 4K images: ~25px).
const SCALES: &[u32] = &[16, 22, 28, 34, 40, 48, 56, 64, 72];
/// Position stride during the sweep (downscaled px).
const STEP: u32 = 2;

/// Build a zero-mean 4-pointed-star (astroid) template of side `s`.
/// `|u|^(2/3) + |v|^(2/3) <= 1` is the astroid whose cusps point along the
/// axes — the sparkle's silhouette. Soft-edged so it matches the glow too.
fn sparkle_template(s: u32) -> Vec<f32> {
    let n = s as usize;
    let mut t = vec![0f32; n * n];
    let denom = (s.max(2) - 1) as f32;
    for j in 0..n {
        for i in 0..n {
            let u = (i as f32 / denom) * 2.0 - 1.0;
            let v = (j as f32 / denom) * 2.0 - 1.0;
            let r = u.abs().powf(2.0 / 3.0) + v.abs().powf(2.0 / 3.0);
            t[j * n + i] = (1.0 - r).clamp(0.0, 1.0);
        }
    }
    let mean = t.iter().sum::<f32>() / (n * n) as f32;
    for v in t.iter_mut() {
        *v -= mean;
    }
    t
}

/// Grayscale (luma) f32 buffer + integral images for O(1) patch sum / sum-sq.
struct Field {
    px: Vec<f32>,
    /// (w+1)*(h+1) summed-area table of px.
    sum: Vec<f64>,
    /// (w+1)*(h+1) summed-area table of px².
    sqsum: Vec<f64>,
}

impl Field {
    fn new(gray: Vec<f32>, w: usize, h: usize) -> Self {
        let stride = w + 1;
        let mut sum = vec![0f64; stride * (h + 1)];
        let mut sqsum = vec![0f64; stride * (h + 1)];
        for y in 0..h {
            let mut row = 0f64;
            let mut sqrow = 0f64;
            for x in 0..w {
                let v = gray[y * w + x] as f64;
                row += v;
                sqrow += v * v;
                sum[(y + 1) * stride + (x + 1)] = sum[y * stride + (x + 1)] + row;
                sqsum[(y + 1) * stride + (x + 1)] = sqsum[y * stride + (x + 1)] + sqrow;
            }
        }
        Field { px: gray, sum, sqsum }
    }

    #[inline]
    fn area(table: &[f64], stride: usize, x: usize, y: usize, s: usize) -> f64 {
        table[(y + s) * stride + (x + s)] - table[y * stride + (x + s)] - table[(y + s) * stride + x]
            + table[y * stride + x]
    }
}

/// Best abs-ZNCC match of the sparkle template within `img`'s bottom-right
/// region, or None if nothing clears MIN_SCORE.
pub fn detect_watermark(img: &RgbImage) -> Option<Detection> {
    let (w, h) = (img.width(), img.height());

    // Bottom-right search region: rightmost/bottom ~48% of the image (the
    // sparkle is always near this corner, and searching only here keeps the
    // false-positive surface and the cost small).
    let rx = (w as f32 * 0.52) as u32;
    let ry = (h as f32 * 0.52) as u32;
    let rw = w - rx;
    let rh = h - ry;
    if rw < 24 || rh < 24 {
        return None;
    }

    let region = imageops::crop_imm(img, rx, ry, rw, rh).to_image();

    // Downscale so the longer side is <= SEARCH_MAX (bounds cost at any res).
    let maxside = rw.max(rh);
    let f = if maxside > SEARCH_MAX { SEARCH_MAX as f32 / maxside as f32 } else { 1.0 };
    let dw = ((rw as f32 * f).round() as u32).max(24);
    let dh = ((rh as f32 * f).round() as u32).max(24);
    let small = if f < 1.0 { imageops::resize(&region, dw, dh, imageops::FilterType::Triangle) } else { region };
    let (dw, dh) = (small.width() as usize, small.height() as usize);

    // luma f32
    let mut gray = vec![0f32; dw * dh];
    for (idx, p) in small.pixels().enumerate() {
        gray[idx] = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
    }
    let field = Field::new(gray, dw, dh);
    let stride = dw + 1;

    let mut best: Option<(f32, usize, usize, u32)> = None; // score, x, y, s
    for &s in SCALES {
        let su = s as usize;
        if su + 1 >= dw || su + 1 >= dh {
            continue;
        }
        let templ = sparkle_template(s);
        let t_energy: f32 = templ.iter().map(|&v| v * v).sum();
        if t_energy < 1e-6 {
            continue;
        }
        let area = (su * su) as f64;
        for y in (0..=dh - su).step_by(STEP as usize) {
            for x in (0..=dw - su).step_by(STEP as usize) {
                let psum = Field::area(&field.sum, stride, x, y, su);
                let psq = Field::area(&field.sqsum, stride, x, y, su);
                // sum((patch-mean)^2) = sumsq - sum^2/area
                let var = psq - psum * psum / area;
                if var < 1e-3 {
                    continue;
                }
                // numerator = sum(patch*templ) (templ is zero-mean).
                let mut num = 0f32;
                for j in 0..su {
                    let base = (y + j) * dw + x;
                    let trow = j * su;
                    for i in 0..su {
                        num += field.px[base + i] * templ[trow + i];
                    }
                }
                let denom = (var as f32 * t_energy).sqrt();
                let score = (num / denom).abs();
                if best.map_or(true, |b| score > b.0) {
                    best = Some((score, x, y, s));
                }
            }
        }
    }

    let (score, bx, by, bs) = best?;
    if score < MIN_SCORE {
        return None;
    }

    // Map downscaled region coords -> full-res image coords.
    let inv = 1.0 / f;
    let cx = rx as f64 + (bx as f64 + bs as f64 / 2.0) * inv as f64;
    let cy = ry as f64 + (by as f64 + bs as f64 / 2.0) * inv as f64;
    let size = (bs as f64 * inv as f64).round() as i64;

    Some(Detection { cx: cx.round() as i64, cy: cy.round() as i64, size, score })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// Paint a synthetic bright sparkle onto a dark background and confirm the
    /// detector localizes it near the painted center with high confidence.
    #[test]
    fn finds_painted_sparkle() {
        let mut img = RgbImage::from_pixel(1200, 1200, Rgb([40, 30, 60]));
        let (scx, scy, half) = (1000i32, 1050i32, 30i32);
        for dy in -half..=half {
            for dx in -half..=half {
                let u = dx as f32 / half as f32;
                let v = dy as f32 / half as f32;
                let r = u.abs().powf(2.0 / 3.0) + v.abs().powf(2.0 / 3.0);
                if r <= 1.0 {
                    let x = (scx + dx) as u32;
                    let y = (scy + dy) as u32;
                    img.put_pixel(x, y, Rgb([235, 235, 245]));
                }
            }
        }
        let d = detect_watermark(&img).expect("should find the sparkle");
        assert!(d.score >= MIN_SCORE, "score too low: {}", d.score);
        assert!((d.cx - scx as i64).abs() <= 12, "cx off: {}", d.cx);
        assert!((d.cy - scy as i64).abs() <= 12, "cy off: {}", d.cy);
    }

    /// A flat/gradient image with no sparkle must not trigger a detection.
    #[test]
    fn rejects_no_watermark() {
        let img = RgbImage::from_fn(1000, 1000, |x, _| Rgb([(x / 4) as u8, 80, 120]));
        assert!(detect_watermark(&img).is_none());
    }
}
