//! Orchestrates the full Gemini-watermark removal pipeline:
//!
//!   1. crop a 512×512 window anchored to the image's bottom-right corner
//!      (`crop::compute_crop_window` — the watermark is always in there)
//!   2. mask just the watermark's bounding box within that window
//!   3. VAE-encode the masked window, run 19 DDIM steps (with classifier-free
//!      guidance) through the UNet, VAE-decode the result
//!   4. feather-blend the decoded window back over the original crop
//!   5. paste that 512×512 window back into the full-resolution image
//!
//! Steps 3 is a direct port of moebius-web's `pipeline.ts` + `ddim.ts`
//! (vendor-validated against the PyTorch reference to mean|Δ|≈0.0022 on the
//! decoded image) — see that project's `notes.md` for the numeric derivation
//! of every constant below. Do not change SCALING_FACTOR, NOISE_OFFSET, or
//! the CFG id convention without re-reading that file; each was determined
//! empirically against the exported graphs, not guessed.

use std::time::Instant;

use ort::value::Tensor;
use rand::{Rng, SeedableRng};

use super::{cpu, crop, detect, imaging, model, scheduler};
use crop::MODEL_SIZE;

const LAT: u32 = 64;
const SCALING_FACTOR: f32 = 0.13025;
const NOISE_OFFSET: f32 = 0.0357;
const HALF_IDS: i64 = 10; // nn.Embedding(20, 3072): [0..10)=cond, [10..20)=uncond

const NUM_STEPS: usize = 20;
const STRENGTH: f64 = 0.99;
const GUIDANCE: f32 = 2.0;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MoebiusError {
    #[error("image decode error: {0}")]
    Decode(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("onnx runtime error: {0}")]
    Onnx(String),
    /// Watermark could not be located (detection below threshold) and the
    /// image isn't in the ≥1024 class where the fixed-corner fallback is
    /// trusted — so we decline to inpaint a guessed region.
    #[error("no watermark detected")]
    NoWatermark,
}

/// Public entry point. Downloads/decoding of the source image already
/// happened in `gemini_watermark::handle`; this takes raw image bytes and
/// returns a PNG-encoded result (the model's output is a fresh synthesis of
/// the masked region, so re-encoding losslessly as PNG rather than trying to
/// preserve the input container format is the safer default).
pub async fn remove_watermark(
    image_bytes: Vec<u8>,
    user_id: i64,
    trace_id: u64,
) -> Result<Vec<u8>, MoebiusError> {
    let cores = cpu::acquire_cpu(user_id, trace_id).await;
    let threads = cores.len().max(1);
    let cores_for_release = cores.clone();

    let result = tokio::task::spawn_blocking(move || {
        cpu::pin_current_thread(&cores, trace_id);
        run_pipeline_sync(&image_bytes, trace_id, threads)
    })
    .await
    .map_err(|e| MoebiusError::Io(format!("spawn_blocking panicked: {e}")))?;

    cpu::release_cpu(cores_for_release, trace_id).await;
    result
}

fn run_pipeline_sync(
    image_bytes: &[u8],
    trace_id: u64,
    threads: usize,
) -> Result<Vec<u8>, MoebiusError> {
    let t_total = Instant::now();

    log_ev!("gwm", trace_id, "moebius_decode_start", "bytes" => image_bytes.len());
    let dynimg =
        image::load_from_memory(image_bytes).map_err(|e| MoebiusError::Decode(e.to_string()))?;
    let mut full = dynimg.into_rgb8();
    let (width, height) = (full.width(), full.height());
    log_ev!("gwm", trace_id, "moebius_decode_done", "width" => width, "height" => height);

    // Locate the sparkle dynamically (its offset depends on aspect ratio).
    let (window, bbox) = match detect::detect_watermark(&full) {
        Some(det) => {
            log_ev!("gwm", trace_id, "moebius_detected",
                "cx" => det.cx, "cy" => det.cy, "size" => det.size,
                "score" => format!("{:.3}", det.score), "=>" => "ok");
            let window = crop::window_around(det.cx, det.cy, width, height);
            let bbox = crop::bbox_from_detection(&det, window);
            (window, bbox)
        }
        None if width >= 1024 && height >= 1024 => {
            log_ev!("gwm", trace_id, "moebius_detect_miss",
                "width" => width, "height" => height, "=>" => "fallback_fixed_corner");
            (
                crop::fixed_crop_window(width, height),
                crop::fixed_bbox_local(),
            )
        }
        None => {
            log_ev!("gwm", trace_id, "moebius_detect_miss",
                "width" => width, "height" => height, "=>" => "no_watermark");
            return Err(MoebiusError::NoWatermark);
        }
    };
    log_ev!("gwm", trace_id, "moebius_window",
        "x0" => window.x0, "y0" => window.y0,
        "bbox" => format!("{},{},{},{}", bbox.x1, bbox.y1, bbox.x2, bbox.y2));

    let crop_img = imaging::extract_window(&full, window);
    let mask512 = imaging::mask_from_bbox(bbox);
    let mask64 = imaging::mask_to_latent(&mask512);

    let crop_chw = imaging::to_chw_norm(&crop_img);
    let masked_chw = imaging::apply_mask_chw(&crop_chw, &mask512);

    let sessions = model::sessions(trace_id, threads).map_err(MoebiusError::Onnx)?;

    log_ev!("gwm", trace_id, "moebius_encode_start");
    let masked_latent = encode(sessions, &masked_chw)?;
    log_ev!("gwm", trace_id, "moebius_encode_done");

    let ddim = scheduler::make_ddim(NUM_STEPS, STRENGTH);
    let plane = (LAT * LAT) as usize;

    let mut latents = randn(4 * plane, trace_id);
    let offsets = randn(4, trace_id ^ 0x9e3779b9);
    for c in 0..4 {
        for p in 0..plane {
            latents[c * plane + p] += NOISE_OFFSET * offsets[c];
        }
    }

    let total_steps = ddim.timesteps.len();
    let t_denoise = Instant::now();
    for (i, &t) in ddim.timesteps.iter().enumerate() {
        let prev_t = ddim.timesteps.get(i + 1).copied().unwrap_or(-1);
        log_ev!("gwm", trace_id, "moebius_step", "i" => i + 1, "total" => total_steps, "t" => t);
        let eps = unet_cfg(sessions, &latents, &mask64, &masked_latent, t, GUIDANCE)?;
        latents = scheduler::ddim_step(&eps, &latents, t, prev_t, &ddim);
    }
    log_ev!("gwm", trace_id, "moebius_denoise_done",
        "steps" => total_steps, "elapsed" => format!("{:.2}s", t_denoise.elapsed().as_secs_f64()));

    log_ev!("gwm", trace_id, "moebius_decode_latent_start");
    let result_chw = decode(sessions, &latents)?;
    let result_img = imaging::chw_to_rgb_image(&result_chw);
    log_ev!("gwm", trace_id, "moebius_decode_latent_done");

    let blended = imaging::feather_blend(&result_img, &crop_img, &mask512);
    imaging::paste_into(&mut full, &blended, window);

    let mut out_bytes: Vec<u8> = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut out_bytes);
        full.write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| MoebiusError::Io(e.to_string()))?;
    }

    log_ev!("gwm", trace_id, "moebius_pipeline_done",
        "out_bytes" => out_bytes.len(),
        "elapsed" => format!("{:.2}s", t_total.elapsed().as_secs_f64()));
    Ok(out_bytes)
}

/// VAE-encode the masked crop and return `mean(moments[:, :4]) * scaling_factor`
/// as a flat (4, 64, 64) latent — the only encoder call the pipeline needs
/// (the un-masked crop's own latent is never used: with strength≈0.99 the
/// initial sample is effectively pure noise, not a noised copy of the input).
fn encode(sessions: &model::Sessions, masked_chw: &[f32]) -> Result<Vec<f32>, MoebiusError> {
    let tensor = Tensor::from_array((
        [1usize, 3, MODEL_SIZE as usize, MODEL_SIZE as usize],
        masked_chw.to_vec(),
    ))
    .map_err(|e| MoebiusError::Onnx(e.to_string()))?;
    let mut sess = crate::sync_util::lock_or_recover(&sessions.encoder);
    let outputs = sess
        .run(ort::inputs!["image" => tensor])
        .map_err(|e| MoebiusError::Onnx(e.to_string()))?;
    let (_, moments) = outputs["moments"]
        .try_extract_tensor::<f32>()
        .map_err(|e| MoebiusError::Onnx(e.to_string()))?;

    let plane = (LAT * LAT) as usize;
    let mut out = vec![0f32; 4 * plane];
    out.copy_from_slice(&moments[..4 * plane]);
    for v in out.iter_mut() {
        *v *= SCALING_FACTOR;
    }
    Ok(out)
}

/// VAE-decode a (4, 64, 64) latent back to a (3, 512, 512) image in [-1, 1].
fn decode(sessions: &model::Sessions, latents: &[f32]) -> Result<Vec<f32>, MoebiusError> {
    let scaled: Vec<f32> = latents.iter().map(|&v| v / SCALING_FACTOR).collect();
    let tensor = Tensor::from_array(([1usize, 4, LAT as usize, LAT as usize], scaled))
        .map_err(|e| MoebiusError::Onnx(e.to_string()))?;
    let mut sess = crate::sync_util::lock_or_recover(&sessions.decoder);
    let outputs = sess
        .run(ort::inputs!["latent" => tensor])
        .map_err(|e| MoebiusError::Onnx(e.to_string()))?;
    let (_, image) = outputs["image"]
        .try_extract_tensor::<f32>()
        .map_err(|e| MoebiusError::Onnx(e.to_string()))?;
    Ok(image.to_vec())
}

/// Assemble the (2,9,64,64) classifier-free-guidance batch (row 0 = unconditional,
/// row 1 = conditional — same 9-channel content, different `input_ids`), run the
/// UNet once, and combine: `cfg = uncond + guidance*(cond - uncond)`.
fn unet_cfg(
    sessions: &model::Sessions,
    latents: &[f32],
    mask64: &[f32],
    masked_latent: &[f32],
    t: i64,
    guidance: f32,
) -> Result<Vec<f32>, MoebiusError> {
    let plane = (LAT * LAT) as usize;

    let mut nine = vec![0f32; 9 * plane];
    nine[0..4 * plane].copy_from_slice(&latents[0..4 * plane]);
    nine[4 * plane..5 * plane].copy_from_slice(mask64);
    nine[5 * plane..9 * plane].copy_from_slice(&masked_latent[0..4 * plane]);

    let mut nine2 = vec![0f32; 2 * 9 * plane];
    nine2[0..9 * plane].copy_from_slice(&nine);
    nine2[9 * plane..18 * plane].copy_from_slice(&nine);

    let mut ids = vec![0i64; 2 * HALF_IDS as usize];
    for i in 0..HALF_IDS {
        ids[i as usize] = HALF_IDS + i; // batch row 0: uncond ids [10..20)
        ids[(HALF_IDS + i) as usize] = i; // batch row 1: cond ids [0..10)
    }
    let ts = vec![t, t];

    let latent_tensor = Tensor::from_array(([2usize, 9, LAT as usize, LAT as usize], nine2))
        .map_err(|e| MoebiusError::Onnx(e.to_string()))?;
    let ts_tensor =
        Tensor::from_array(([2usize], ts)).map_err(|e| MoebiusError::Onnx(e.to_string()))?;
    let ids_tensor = Tensor::from_array(([2usize, HALF_IDS as usize], ids))
        .map_err(|e| MoebiusError::Onnx(e.to_string()))?;

    let mut sess = crate::sync_util::lock_or_recover(&sessions.unet);
    let outputs = sess
        .run(ort::inputs![
            "latent" => latent_tensor,
            "timesteps" => ts_tensor,
            "input_ids" => ids_tensor,
        ])
        .map_err(|e| MoebiusError::Onnx(e.to_string()))?;
    let (_, noise) = outputs["noise"]
        .try_extract_tensor::<f32>()
        .map_err(|e| MoebiusError::Onnx(e.to_string()))?;

    let n = 4 * plane;
    let mut cfg = vec![0f32; n];
    for i in 0..n {
        let u = noise[i];
        let c = noise[n + i];
        cfg[i] = u + guidance * (c - u);
    }
    Ok(cfg)
}

/// Seeded standard-normal draws (Box–Muller). Diffusion quality is robust to
/// the particular noise draw (see moebius-web notes.md "Parity strategy"), so
/// bit-parity with the JS reference's RNG isn't required — only that the
/// draws are seeded (reproducible per trace, useful when debugging a bad
/// output) and are actually Gaussian.
fn randn(n: usize, seed: u64) -> Vec<f32> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let u: f64 = rng.gen_range(f64::EPSILON..1.0);
        let v: f64 = rng.gen_range(0.0..1.0);
        let r = (-2.0 * u.ln()).sqrt();
        out.push((r * (2.0 * std::f64::consts::PI * v).cos()) as f32);
        if out.len() < n {
            out.push((r * (2.0 * std::f64::consts::PI * v).sin()) as f32);
        }
    }
    out.truncate(n);
    out
}

#[cfg(test)]
mod integration {
    use super::*;

    /// Loads the real ONNX graphs and runs all 19 DDIM steps on a synthetic
    /// image — a full end-to-end sanity check that the tensor shapes/names
    /// wired up in `encode`/`unet_cfg`/`decode` actually match the exported
    /// graphs. Slow (real CPU inference) and requires
    /// `files/models/moebius/*.onnx` on disk, so it's `#[ignore]`d by
    /// default; run explicitly with `cargo test ... -- --ignored`.
    #[test]
    #[ignore]
    fn full_pipeline_runs_on_synthetic_image() {
        let img = image::RgbImage::from_fn(1024, 1024, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();

        // No detectable sparkle, but ≥1024 in both dims -> fixed-corner
        // fallback path runs and still produces a valid same-size image.
        let out = run_pipeline_sync(&bytes, 999_999, 1).expect("pipeline should succeed");
        let decoded = image::load_from_memory(&out).expect("output should be a valid image");
        assert_eq!(decoded.width(), 1024);
        assert_eq!(decoded.height(), 1024);
    }
}
