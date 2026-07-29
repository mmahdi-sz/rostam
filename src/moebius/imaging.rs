//! Pixel-level glue between `image::RgbImage` and the model's tensor layout.
//! Mirrors moebius-web's `imaging.ts` (the vendor-validated reference) so the
//! numeric conventions (channel order, [-1,1] normalization, nearest-neighbor
//! mask downsample, feather blend) match exactly.

use image::{Rgb, RgbImage};

use super::crop::{CropWindow, MODEL_SIZE, WatermarkBBox};

const LAT: u32 = 64; // 512 / 8 (VAE downsample factor)

/// Sample `img` at (x, y), clamping out-of-bounds coordinates to the nearest
/// edge pixel. This is what lets `extract_window` double as both crop *and*
/// pad: coordinates outside the source just replicate the edge.
fn sample_clamped(img: &RgbImage, x: i64, y: i64) -> Rgb<u8> {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let cx = x.clamp(0, w - 1) as u32;
    let cy = y.clamp(0, h - 1) as u32;
    *img.get_pixel(cx, cy)
}

/// Extract the model-sized (512×512) working window from the source image at
/// `window`, edge-clamping any coordinates that fall outside the source.
pub fn extract_window(img: &RgbImage, window: CropWindow) -> RgbImage {
    let mut out = RgbImage::new(MODEL_SIZE, MODEL_SIZE);
    for j in 0..MODEL_SIZE as i64 {
        for i in 0..MODEL_SIZE as i64 {
            let px = sample_clamped(img, window.x0 + i, window.y0 + j);
            out.put_pixel(i as u32, j as u32, px);
        }
    }
    out
}

/// Write `window_result` (512×512) back into `dest` at `window`'s origin,
/// silently dropping any part of the window that fell outside `dest` when it
/// was extracted (i.e. the edge-clamped padding never gets written back).
pub fn paste_into(dest: &mut RgbImage, window_result: &RgbImage, window: CropWindow) {
    let (w, h) = (dest.width() as i64, dest.height() as i64);
    for j in 0..MODEL_SIZE as i64 {
        let dy = window.y0 + j;
        if dy < 0 || dy >= h {
            continue;
        }
        for i in 0..MODEL_SIZE as i64 {
            let dx = window.x0 + i;
            if dx < 0 || dx >= w {
                continue;
            }
            let px = *window_result.get_pixel(i as u32, j as u32);
            dest.put_pixel(dx as u32, dy as u32, px);
        }
    }
}

/// HWC u8 image -> CHW f32 in [-1, 1]. (3, 512, 512)
pub fn to_chw_norm(img: &RgbImage) -> Vec<f32> {
    let plane = (MODEL_SIZE * MODEL_SIZE) as usize;
    let mut out = vec![0f32; 3 * plane];
    for (idx, px) in img.pixels().enumerate() {
        out[idx] = (px[0] as f32 / 255.0) * 2.0 - 1.0;
        out[plane + idx] = (px[1] as f32 / 255.0) * 2.0 - 1.0;
        out[2 * plane + idx] = (px[2] as f32 / 255.0) * 2.0 - 1.0;
    }
    out
}

/// Binary 512×512 mask (1.0 = hole to inpaint) from a rectangular bbox.
pub fn mask_from_bbox(bbox: WatermarkBBox) -> Vec<f32> {
    let mut out = vec![0f32; (MODEL_SIZE * MODEL_SIZE) as usize];
    for y in bbox.y1..bbox.y2 {
        for x in bbox.x1..bbox.x2 {
            out[(y * MODEL_SIZE + x) as usize] = 1.0;
        }
    }
    out
}

/// masked_chw[c] = chw[c] * (1 - mask) — zeroes the hole per channel.
pub fn apply_mask_chw(chw: &[f32], mask512: &[f32]) -> Vec<f32> {
    let plane = (MODEL_SIZE * MODEL_SIZE) as usize;
    let mut out = vec![0f32; chw.len()];
    for c in 0..3 {
        for p in 0..plane {
            out[c * plane + p] = chw[c * plane + p] * (1.0 - mask512[p]);
        }
    }
    out
}

/// Nearest-neighbor downsample 512×512 -> 64×64 (top-left of each 8×8 block),
/// matching PyTorch's default nearest-mode resize used to build the mask
/// channel fed into the 9-channel UNet input.
pub fn mask_to_latent(mask512: &[f32]) -> Vec<f32> {
    let ratio = MODEL_SIZE / LAT; // 8
    let mut out = vec![0f32; (LAT * LAT) as usize];
    for y in 0..LAT {
        for x in 0..LAT {
            out[(y * LAT + x) as usize] = mask512[((y * ratio) * MODEL_SIZE + x * ratio) as usize];
        }
    }
    out
}

/// Decoder output CHW f32 in [-1,1] -> HWC u8 RgbImage (512×512), clipped.
pub fn chw_to_rgb_image(chw: &[f32]) -> RgbImage {
    let plane = (MODEL_SIZE * MODEL_SIZE) as usize;
    let mut out = RgbImage::new(MODEL_SIZE, MODEL_SIZE);
    for p in 0..plane {
        let x = (p as u32) % MODEL_SIZE;
        let y = (p as u32) / MODEL_SIZE;
        let mut px = [0u8; 3];
        for (c, slot) in px.iter_mut().enumerate() {
            let v = ((chw[c * plane + p] + 1.0) / 2.0).clamp(0.0, 1.0);
            *slot = (v * 255.0).round() as u8;
        }
        out.put_pixel(x, y, Rgb(px));
    }
    out
}

/// 3-pass box blur (cheap Gaussian approximation) of a 512×512 f32 field,
/// matching moebius-web's `filter: blur(3px)` feather used before pasting
/// the inpainted region back over the original crop.
fn box_blur_512(field: &[f32], radius: i64) -> Vec<f32> {
    let n = MODEL_SIZE as i64;
    let mut buf = field.to_vec();
    for _pass in 0..3 {
        // horizontal
        let mut h = vec![0f32; buf.len()];
        for y in 0..n {
            for x in 0..n {
                let mut sum = 0f32;
                let mut count = 0f32;
                for dx in -radius..=radius {
                    let sx = x + dx;
                    if sx >= 0 && sx < n {
                        sum += buf[(y * n + sx) as usize];
                        count += 1.0;
                    }
                }
                h[(y * n + x) as usize] = sum / count;
            }
        }
        // vertical
        let mut v = vec![0f32; buf.len()];
        for y in 0..n {
            for x in 0..n {
                let mut sum = 0f32;
                let mut count = 0f32;
                for dy in -radius..=radius {
                    let sy = y + dy;
                    if sy >= 0 && sy < n {
                        sum += h[(sy * n + x) as usize];
                        count += 1.0;
                    }
                }
                v[(y * n + x) as usize] = sum / count;
            }
        }
        buf = v;
    }
    buf
}

/// Feather-blend the model's result over the *original* crop using a blurred
/// version of the binary mask: `out = result*blur(mask) + original*(1-blur(mask))`.
/// Everywhere far from the mask, `blur(mask) ≈ 0`, so those pixels come back
/// bit-identical to `original` — which is what makes it safe to paste the
/// entire 512×512 window back into the full-res image with no extra seam
/// blending at the crop boundary.
pub fn feather_blend(result: &RgbImage, original: &RgbImage, mask512: &[f32]) -> RgbImage {
    let blurred = box_blur_512(mask512, 3);
    let mut out = RgbImage::new(MODEL_SIZE, MODEL_SIZE);
    for y in 0..MODEL_SIZE {
        for x in 0..MODEL_SIZE {
            let m = blurred[(y * MODEL_SIZE + x) as usize].clamp(0.0, 1.0);
            let r = result.get_pixel(x, y);
            let o = original.get_pixel(x, y);
            let blend = |a: u8, b: u8| ((a as f32) * m + (b as f32) * (1.0 - m)).round() as u8;
            out.put_pixel(
                x,
                y,
                Rgb([blend(r[0], o[0]), blend(r[1], o[1]), blend(r[2], o[2])]),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_normalize_stays_in_range() {
        let img = RgbImage::from_pixel(4, 4, Rgb([10, 200, 128]));
        let chw = to_chw_norm(&img);
        assert!(chw.iter().all(|&v| (-1.0..=1.0).contains(&v)));
    }

    #[test]
    fn mask_downsample_shrinks_by_8() {
        let bbox = WatermarkBBox {
            x1: 400,
            y1: 400,
            x2: 480,
            y2: 480,
        };
        let mask = mask_from_bbox(bbox);
        let lat = mask_to_latent(&mask);
        assert_eq!(lat.len(), (LAT * LAT) as usize);
        assert!(lat.iter().any(|&v| v > 0.0));
    }

    #[test]
    fn feather_blend_matches_original_far_from_mask() {
        let orig = RgbImage::from_pixel(MODEL_SIZE, MODEL_SIZE, Rgb([1, 2, 3]));
        let result = RgbImage::from_pixel(MODEL_SIZE, MODEL_SIZE, Rgb([250, 251, 252]));
        let bbox = WatermarkBBox {
            x1: 400,
            y1: 400,
            x2: 480,
            y2: 480,
        };
        let mask = mask_from_bbox(bbox);
        let out = feather_blend(&result, &orig, &mask);
        // top-left corner is far from the mask -> should be ~unchanged original.
        assert_eq!(*out.get_pixel(0, 0), Rgb([1, 2, 3]));
    }
}
