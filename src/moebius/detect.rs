//! Gemini sparkle watermark localization using symmetry, correlation, concavity, and contrast signals.

use image::{RgbImage, imageops};

/// A located watermark, in full-resolution image coordinates.
#[derive(Debug, Clone, Copy)]
pub struct Detection {
    pub cx: i64,
    pub cy: i64,
    /// Side length of the (square) sparkle box in full-res pixels.
    pub size: i64,
    pub score: f32,
}

/// Fraction of the image (from top-left) excluded from the search — the sparkle
/// is always in the bottom-right, so we scan the bottom-right ~50%.
const SEARCH_FRAC: f32 = 0.50;
/// The bottom-right region is downscaled so its longer side is ≤ this before the
/// (dense) candidate scan, bounding cost at any resolution. 720 leaves common
/// (≤~1440-wide) images unscaled while capping 4K work.
const SEARCH_MAX: u32 = 720;
/// Half-window (px, in the downscaled scan space) for the candidate symmetry
/// probe. Small: sparkle symmetry peaks in a tight window before surrounding
/// texture leaks in.
const SYM_H1: usize = 16;
/// Grid stride for the dense candidate scan (downscaled px). Small enough that
/// some grid point lands within the sparkle's high-symmetry core.
const GSTEP: usize = 3;
/// Candidate collection threshold (below the final `T_SYM` gate, so a slightly
/// off-center grid hit still enters and is then refined onto the true center).
const CAND_T: f32 = 0.45;
/// Non-max-suppression radius (downscaled px) between kept candidates.
const NMS: i64 = 14;
/// Cap on candidates carried into the (more expensive) verification stage.
const MAX_CAND: usize = 25;

// --- verification gates (all must pass) ---
const T_SYM: f32 = 0.55;
const T_SHAPE: f32 = 0.30;
const T_WEDGE: f32 = 0.15;
/// Minimum overlay contrast in gray levels — the sparkle visibly lightens (or
/// darkens) its patch; smooth low-energy regions fall below this.
const T_ABSBRIGHT: f32 = 12.0;
/// Combined-score floor for a detection to be reported (also the `score` field).
const MIN_SCORE: f32 = 0.9;

/// Full-res analysis scales (half-window px). Gemini renders a ~48px sparkle for
/// images with a dimension ≤1024 and ~96px when both exceed 1024; the range
/// spans both classes plus tolerance, and the best-fitting scale is chosen per
/// candidate.
fn scales(w: u32, h: u32) -> &'static [usize] {
    if w.min(h) > 1024 {
        &[18, 24, 30, 40, 52]
    } else {
        &[14, 18, 24, 30]
    }
}

fn luma_of(img: &RgbImage) -> Vec<f32> {
    img.pixels()
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect()
}

/// Separable box blur with edge-clamp (replicate). `size` is forced odd.
fn box_blur(src: &[f32], w: usize, h: usize, size: usize) -> Vec<f32> {
    let r = ((size.max(1)) | 1) / 2;
    let win = (2 * r + 1) as f32;
    let clamp = |v: isize, hi: usize| v.clamp(0, hi as isize - 1) as usize;
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0f32;
            for k in 0..=2 * r {
                s += src[y * w + clamp(x as isize + k as isize - r as isize, w)];
            }
            tmp[y * w + x] = s / win;
        }
    }
    let mut out = vec![0f32; w * h];
    for x in 0..w {
        for y in 0..h {
            let mut s = 0f32;
            for k in 0..=2 * r {
                s += tmp[clamp(y as isize + k as isize - r as isize, h) * w + x];
            }
            out[y * w + x] = s / win;
        }
    }
    out
}

/// High-pass = luma − box-blur(luma): strips the background so downstream
/// signals key on the overlay rather than the scene content.
fn high_pass(src: &[f32], w: usize, h: usize, box_size: usize) -> Vec<f32> {
    let bb = box_blur(src, w, h, box_size);
    src.iter().zip(&bb).map(|(a, b)| a - b).collect()
}

/// Copy the (2·half)² block centered at (cx,cy) out of `luma` and return its
/// high-pass. Caller guarantees the block is in-bounds.
fn patch_hp(luma: &[f32], w: usize, cx: usize, cy: usize, half: usize) -> Vec<f32> {
    let n = 2 * half;
    let mut p = vec![0f32; n * n];
    for j in 0..n {
        let row = (cy - half + j) * w + (cx - half);
        p[j * n..j * n + n].copy_from_slice(&luma[row..row + n]);
    }
    let bb = box_blur(&p, n, n, (half as f32 * 1.2) as usize | 1);
    for k in 0..n * n {
        p[k] -= bb[k];
    }
    p
}

/// Mean over the 6 non-identity dihedral (D4) self-correlations of an n×n patch
/// read from `buf` at (x0,y0) with row stride `stride`. 1.0 = perfectly
/// symmetric under every mirror/rotation, ~0 = no symmetry. Sign-agnostic.
fn d4_sym(buf: &[f32], stride: usize, x0: usize, y0: usize, n: usize) -> f32 {
    let at = |c: usize, r: usize| buf[(y0 + r) * stride + (x0 + c)];
    let mut sum = 0f32;
    for r in 0..n {
        for c in 0..n {
            sum += at(c, r);
        }
    }
    let mean = sum / (n * n) as f32;
    let g = |c: usize, r: usize| at(c, r) - mean;
    let (mut base, mut s1, mut s2, mut s3, mut s4, mut s5, mut s6) =
        (1e-9f32, 0f32, 0f32, 0f32, 0f32, 0f32, 0f32);
    for r in 0..n {
        for c in 0..n {
            let v = g(c, r);
            base += v * v;
            s1 += v * g(n - 1 - c, r); // mirror x
            s2 += v * g(c, n - 1 - r); // mirror y
            s3 += v * g(n - 1 - c, n - 1 - r); // rot180
            s4 += v * g(r, c); // transpose (main diagonal)
            s5 += v * g(n - 1 - r, n - 1 - c); // anti-diagonal
            s6 += v * g(r, n - 1 - c); // rot90
        }
    }
    (s1 + s2 + s3 + s4 + s5 + s6) / (6.0 * base)
}

/// Soft 4-pointed-star (astroid) silhouette of side n, values in [0,1].
fn astroid_raw(n: usize) -> Vec<f32> {
    let d = (n.max(2) - 1) as f32;
    let mut t = vec![0f32; n * n];
    for j in 0..n {
        for i in 0..n {
            let u = (i as f32 / d) * 2.0 - 1.0;
            let v = (j as f32 / d) * 2.0 - 1.0;
            let rr = u.abs().powf(2.0 / 3.0) + v.abs().powf(2.0 / 3.0);
            t[j * n + i] = (1.0 - rr).clamp(0.0, 1.0);
        }
    }
    t
}

/// Overlay contrast: mean residual inside the star silhouette minus outside.
/// Sign tells sparkle polarity (bright on dark bg > 0, dark on light bg < 0).
fn bright(hp: &[f32], n: usize) -> f32 {
    let a = astroid_raw(n);
    let (mut cs, mut cn, mut gs, mut gn) = (0f32, 0usize, 0f32, 0usize);
    for k in 0..n * n {
        if a[k] > 0.25 {
            cs += hp[k];
            cn += 1;
        } else {
            gs += hp[k];
            gn += 1;
        }
    }
    cs / cn.max(1) as f32 - gs / gn.max(1) as f32
}

/// Normalized correlation of the (sign-corrected) residual with the astroid
/// silhouette — how well the residual matches the 4-pointed shape.
fn signed_shape(hp: &[f32], n: usize, sign: f32) -> f32 {
    let a0 = astroid_raw(n);
    let amean = a0.iter().sum::<f32>() / (n * n) as f32;
    let rmean = sign * hp.iter().sum::<f32>() / (n * n) as f32;
    let (mut num, mut rr, mut aa) = (0f32, 0f32, 0f32);
    for k in 0..n * n {
        let r = hp[k] * sign - rmean;
        let a = a0[k] - amean;
        num += r * a;
        rr += r * r;
        aa += a * a;
    }
    num / ((rr * aa).sqrt() + 1e-9)
}

/// Ray-vs-gap contrast: mean residual along the 4 axes minus along the 4
/// diagonals (on a mid-radius ring), normalized by ring RMS. A star has bright
/// rays and dark diagonal gaps (>0); a round blob has neither (≈0) — the signal
/// that tells a sparkle from a symmetric highlight.
fn signed_wedge(hp: &[f32], n: usize, sign: f32) -> f32 {
    let c = (n as f32 - 1.0) / 2.0;
    let half = n as f32 / 2.0;
    let (mut ax_s, mut ax_n, mut dg_s, mut dg_n, mut band_sq, mut band_n) =
        (0f32, 0usize, 0f32, 0usize, 0f32, 0usize);
    for j in 0..n {
        for i in 0..n {
            let dx = i as f32 - c;
            let dy = j as f32 - c;
            let rad = (dx * dx + dy * dy).sqrt() / half;
            if !(0.5..=0.88).contains(&rad) {
                continue;
            }
            let mut ang = dy.atan2(dx).to_degrees() % 180.0;
            if ang < 0.0 {
                ang += 180.0;
            }
            let dax = (ang - 0.0)
                .abs()
                .min((ang - 90.0).abs())
                .min((ang - 180.0).abs());
            let ddg = (ang - 45.0).abs().min((ang - 135.0).abs());
            let v = hp[j * n + i];
            band_sq += v * v;
            band_n += 1;
            let vs = v * sign;
            if dax <= 22.0 {
                ax_s += vs;
                ax_n += 1;
            }
            if ddg <= 22.0 {
                dg_s += vs;
                dg_n += 1;
            }
        }
    }
    if ax_n < 4 || dg_n < 4 || band_n == 0 {
        return 0.0;
    }
    let rms = (band_sq / band_n as f32).sqrt() + 1e-6;
    (ax_s / ax_n as f32 - dg_s / dg_n as f32) / rms
}

/// Best multi-signal match of the Gemini sparkle in `img`'s bottom-right
/// region, or None if nothing clears every gate.
pub fn detect_watermark(img: &RgbImage) -> Option<Detection> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let luma = luma_of(img);

    let rx = (w as f32 * SEARCH_FRAC) as usize;
    let ry = (h as f32 * SEARCH_FRAC) as usize;
    let (rw, rh) = (w - rx, h - ry);
    if rw < 32 || rh < 32 {
        return None;
    }

    // ---- Stage 1: symmetric candidates on the downscaled region ----
    let maxside = rw.max(rh) as u32;
    let f = if maxside > SEARCH_MAX {
        SEARCH_MAX as f32 / maxside as f32
    } else {
        1.0
    };
    let region = imageops::crop_imm(img, rx as u32, ry as u32, rw as u32, rh as u32).to_image();
    let small = if f < 1.0 {
        let (dw, dh) = (
            (rw as f32 * f).round().max(32.0) as u32,
            (rh as f32 * f).round().max(32.0) as u32,
        );
        imageops::resize(&region, dw, dh, imageops::FilterType::Triangle)
    } else {
        region
    };
    let (dw, dh) = (small.width() as usize, small.height() as usize);
    let sg = luma_of(&small);
    let hp1 = high_pass(&sg, dw, dh, (SYM_H1 as f32 * 1.2) as usize | 1);

    let mut cands: Vec<(f32, usize, usize)> = Vec::new();
    let mut cy = SYM_H1;
    while cy + SYM_H1 < dh {
        let mut cx = SYM_H1;
        while cx + SYM_H1 < dw {
            let s = d4_sym(&hp1, dw, cx - SYM_H1, cy - SYM_H1, 2 * SYM_H1);
            if s >= CAND_T {
                cands.push((s, cx, cy));
            }
            cx += GSTEP;
        }
        cy += GSTEP;
    }
    cands.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut kept: Vec<(usize, usize)> = Vec::new();
    for &(_, cx, cy) in &cands {
        let far = kept.iter().all(|&(kx, ky)| {
            let (dx, dy) = (cx as i64 - kx as i64, cy as i64 - ky as i64);
            dx * dx + dy * dy > NMS * NMS
        });
        if far {
            kept.push((cx, cy));
            if kept.len() >= MAX_CAND {
                break;
            }
        }
    }

    // ---- Stage 2: full-res multi-scale verification ----
    let hpf = high_pass(&luma, w, h, (SYM_H1 as f32 * 1.2) as usize | 1);
    let scl = scales(img.width(), img.height());
    let (fx, fy) = (rw as f32 / dw as f32, rh as f32 / dh as f32);

    let mut best: Option<Detection> = None;
    let mut best_comb = MIN_SCORE;

    for &(cxs, cys) in &kept {
        let cx0 = rx + (cxs as f32 * fx).round() as usize;
        let cy0 = ry + (cys as f32 * fy).round() as usize;

        // Refine center by maximizing symmetry (strided read from full-res HP).
        let mut rbest: Option<(f32, usize, usize)> = None;
        for dyi in -5i64..=5 {
            for dxi in -5i64..=5 {
                let (x, y) = (cx0 as i64 + dxi, cy0 as i64 + dyi);
                if x - SYM_H1 as i64 <= 0
                    || y - SYM_H1 as i64 <= 0
                    || x + SYM_H1 as i64 >= w as i64
                    || y + SYM_H1 as i64 >= h as i64
                {
                    continue;
                }
                let (x, y) = (x as usize, y as usize);
                let s = d4_sym(&hpf, w, x - SYM_H1, y - SYM_H1, 2 * SYM_H1);
                if rbest.map_or(true, |b| s > b.0) {
                    rbest = Some((s, x, y));
                }
            }
        }
        let (sym, x, y) = match rbest {
            Some(v) if v.0 >= T_SYM => v,
            _ => continue,
        };

        for &hh in scl {
            if x < hh || y < hh || x + hh >= w || y + hh >= h {
                continue;
            }
            let n = 2 * hh;
            let hp = patch_hp(&luma, w, x, y, hh);
            let br = bright(&hp, n);
            let sign = if br >= 0.0 { 1.0 } else { -1.0 };
            let shape = signed_shape(&hp, n, sign);
            let wedge = signed_wedge(&hp, n, sign);
            if shape >= T_SHAPE && wedge >= T_WEDGE && br.abs() >= T_ABSBRIGHT {
                let comb = sym + 0.5 * shape.clamp(0.0, 1.0) + 0.5 * wedge.tanh();
                if comb > best_comb {
                    best_comb = comb;
                    best = Some(Detection {
                        cx: x as i64,
                        cy: y as i64,
                        size: (hh as f32 * 1.7).round() as i64,
                        score: comb,
                    });
                }
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// Paint a bright astroid sparkle onto a dark background and confirm the
    /// detector localizes it near the painted center and clears the gate.
    #[test]
    fn finds_painted_sparkle() {
        let mut img = RgbImage::from_pixel(1200, 1200, Rgb([40, 30, 60]));
        let (scx, scy, half) = (1000i32, 1050i32, 30i32);
        for dy in -half..=half {
            for dx in -half..=half {
                let u = dx as f32 / half as f32;
                let v = dy as f32 / half as f32;
                if u.abs().powf(2.0 / 3.0) + v.abs().powf(2.0 / 3.0) <= 1.0 {
                    img.put_pixel((scx + dx) as u32, (scy + dy) as u32, Rgb([235, 235, 245]));
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

    /// A bright *round* blob is symmetric and astroid-ish but has no rays — the
    /// wedge/concavity gate must reject it (guards the disk-vs-star signal).
    #[test]
    fn rejects_bright_disk() {
        let mut img = RgbImage::from_pixel(1200, 1200, Rgb([40, 30, 60]));
        let (scx, scy, rad) = (1000i32, 1050i32, 26i32);
        for dy in -rad..=rad {
            for dx in -rad..=rad {
                if dx * dx + dy * dy <= rad * rad {
                    img.put_pixel((scx + dx) as u32, (scy + dy) as u32, Rgb([235, 235, 245]));
                }
            }
        }
        assert!(
            detect_watermark(&img).is_none(),
            "bright disk must not be detected as a sparkle"
        );
    }
}
