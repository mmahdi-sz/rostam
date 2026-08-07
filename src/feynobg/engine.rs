use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ort::session::{Session, builder::GraphOptimizationLevel};
use ort::value::Tensor;

const MODEL_PATH: &str = "files/models/feynobg/feynobg_int8_dynamic.onnx";
const INPUT_SIZE: usize = 1024;

/// How long after the last inference to keep the session alive before unloading.
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// Idle-unloading session holder
//
// The model weighs ~1.3 GB. Keeping it permanently loaded means the process
// holds >8 GB RSS even when nobody is using nobg. Instead we:
//   • load the session on the first request after it was absent
//   • reset the idle timer on every request
//   • a background task drops the session after SESSION_IDLE_TIMEOUT of
//     inactivity, then calls malloc_trim so jemalloc/glibc returns the
//     pages to the kernel immediately
// ---------------------------------------------------------------------------
struct SessionHolder {
    session: Option<Session>,
    last_used: Instant,
}

impl SessionHolder {
    fn new() -> Self {
        Self {
            session: None,
            last_used: Instant::now(),
        }
    }

    /// Return a mutable reference to the inner session, loading it if needed.
    fn get_or_load(&mut self, threads: usize) -> Result<&mut Session, String> {
        if self.session.is_none() {
            eprintln!("[feynobg] Loading ONNX session (first use / after idle unload)");
            let sess = (|| -> ort::Result<Session> {
                Session::builder()?
                    .with_optimization_level(GraphOptimizationLevel::Level3)?
                    .with_intra_threads(threads.max(1))?
                    .with_memory_pattern(false)?
                    .commit_from_file(MODEL_PATH)
            })()
            .map_err(|e| format!("load model {MODEL_PATH}: {e}"))?;
            self.session = Some(sess);
        }
        self.last_used = Instant::now();
        Ok(self.session.as_mut().unwrap())
    }

    /// Drop the session and return true if it was actually loaded.
    fn unload(&mut self) -> bool {
        if self.session.is_some() {
            self.session = None;
            eprintln!("[feynobg] ONNX session unloaded (idle timeout)");
            return true;
        }
        false
    }

    fn is_idle(&self) -> bool {
        self.session.is_some() && self.last_used.elapsed() >= SESSION_IDLE_TIMEOUT
    }
}

// Single global holder, shared between the inference path and the reaper task.
static SESSION_HOLDER: std::sync::OnceLock<Arc<Mutex<SessionHolder>>> = std::sync::OnceLock::new();

fn session_holder() -> &'static Arc<Mutex<SessionHolder>> {
    SESSION_HOLDER.get_or_init(|| Arc::new(Mutex::new(SessionHolder::new())))
}

/// Spawn a background task that periodically checks whether the session has
/// been idle long enough to unload. Call this once at startup (e.g. from
/// `app::run`), but it is safe to call multiple times (only the first wins).
pub fn spawn_session_reaper() {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        return; // already running
    }

    let holder = session_holder().clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;

            let unloaded = {
                let mut h = holder.lock().unwrap_or_else(|e| e.into_inner());
                if h.is_idle() { h.unload() } else { false }
            };

            if unloaded {
                // Ask the allocator to return freed pages to the OS right now.
                trim_allocator();
            }
        }
    });
}

/// Hint to the memory allocator to release unused pages back to the OS.
///
/// tikv-jemallocator is wired as `#[global_allocator]` (not a cargo feature),
/// so we simply call `malloc_trim(0)` which works for both jemalloc and glibc:
/// jemalloc intercepts it and calls `MADV_DONTNEED` on its dirty pages,
/// returning them to the kernel immediately.
fn trim_allocator() {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::malloc_trim(0);
    }
    eprintln!("[feynobg] malloc_trim called after session unload");
}

// ----------------------------------------------------------------------------
// Bilinear resize helper for float32 mask buffers
// ----------------------------------------------------------------------------
fn bilinear_resize(src: &[f32], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<f32> {
    let mut dst = vec![0.0; (dst_w * dst_h) as usize];
    let x_ratio = ((src_w - 1) as f32) / ((dst_w - 1) as f32).max(1.0);
    let y_ratio = ((src_h - 1) as f32) / ((dst_h - 1) as f32).max(1.0);

    for i in 0..dst_h {
        let y = (i as f32) * y_ratio;
        let y_l = y.floor() as u32;
        let y_h = (y_l + 1).min(src_h - 1);
        let y_weight = y - (y_l as f32);

        for j in 0..dst_w {
            let x = (j as f32) * x_ratio;
            let x_l = x.floor() as u32;
            let x_h = (x_l + 1).min(src_w - 1);
            let x_weight = x - (x_l as f32);

            let val_a = src[(y_l * src_w + x_l) as usize];
            let val_b = src[(y_l * src_w + x_h) as usize];
            let val_c = src[(y_h * src_w + x_l) as usize];
            let val_d = src[(y_h * src_w + x_h) as usize];

            let val = val_a * (1.0 - x_weight) * (1.0 - y_weight)
                + val_b * x_weight * (1.0 - y_weight)
                + val_c * (1.0 - x_weight) * y_weight
                + val_d * x_weight * y_weight;

            dst[(i * dst_w + j) as usize] = val;
        }
    }
    dst
}

// ----------------------------------------------------------------------------
// Main FeyNobg background removal function
// ----------------------------------------------------------------------------
pub async fn run_nobg(
    input_path: &Path,
    output_path: &Path,
    user_id: i64,
    trace_id: u64,
) -> Result<Duration, String> {
    let start_time = Instant::now();
    log_ev!("feynobg", trace_id, "engine_start",
        "input" => input_path.display(), "model" => MODEL_PATH);

    // Acquire CPU core allocation from CPU Broker
    let cores = crate::moebius::cpu::acquire_cpu(user_id, trace_id).await;
    log_ev!("feynobg", trace_id, "cpu_acquired", "cores" => format!("{cores:?}"));

    let num_threads = cores.len().max(1);
    let input_path_buf = input_path.to_path_buf();
    let output_path_buf = output_path.to_path_buf();
    let holder = session_holder().clone();

    let process_res = tokio::task::spawn_blocking(move || -> Result<(), String> {
        // --- 1. Load image ---
        let t_load = Instant::now();
        let img = image::open(&input_path_buf).map_err(|e| format!("image open: {e}"))?;
        let (orig_w, orig_h) = (img.width(), img.height());
        eprintln!(
            "[feynobg trace={trace_id} event=image_loaded] w={orig_w} h={orig_h} load_ms={:.0}",
            t_load.elapsed().as_secs_f64() * 1000.0
        );

        // --- 2. Preprocess ---
        let t_pre = Instant::now();
        let rgb8 = img.to_rgb8();

        // Resize image to 1024x1024 for model input
        let resized = image::imageops::resize(
            &rgb8,
            INPUT_SIZE as u32,
            INPUT_SIZE as u32,
            image::imageops::FilterType::CatmullRom,
        );

        // ImageNet normalization
        let mean = [0.485f32, 0.456, 0.406];
        let std = [0.229f32, 0.224, 0.225];

        let plane_size = INPUT_SIZE * INPUT_SIZE;
        let mut chw_input = vec![0f32; 3 * plane_size];

        for (idx, pixel) in resized.pixels().enumerate() {
            let r = (pixel[0] as f32 / 255.0 - mean[0]) / std[0];
            let g = (pixel[1] as f32 / 255.0 - mean[1]) / std[1];
            let b = (pixel[2] as f32 / 255.0 - mean[2]) / std[2];

            chw_input[idx] = r;
            chw_input[plane_size + idx] = g;
            chw_input[2 * plane_size + idx] = b;
        }

        eprintln!(
            "[feynobg trace={trace_id} event=preprocess_done] ms={:.0}",
            t_pre.elapsed().as_secs_f64() * 1000.0
        );

        // --- 3. ONNX Inference ---
        let t_inf = Instant::now();

        let input_tensor = Tensor::from_array(([1usize, 3, INPUT_SIZE, INPUT_SIZE], chw_input))
            .map_err(|e| format!("tensor create: {e}"))?;

        // Lock the session for the duration of inference only.
        let mut h = holder.lock().map_err(|e| format!("session lock: {e}"))?;
        let sess = h.get_or_load(num_threads)?;

        let input_name = sess.inputs()[0].name().to_string();
        let output_name = sess.outputs()[0].name().to_string();

        let outputs = sess
            .run(ort::inputs![input_name.as_str() => input_tensor])
            .map_err(|e| format!("onnx run: {e}"))?;

        let (_, output_data) = outputs[output_name.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("extract output: {e}"))?;

        let mask_1024: Vec<f32> = output_data.to_vec();

        // Drop `outputs` and release the session lock before postprocessing
        // so the mutex is held for the minimum time and jemalloc can see the
        // freed ORT output buffers.
        drop(outputs);
        drop(h);

        let inf_ms = t_inf.elapsed().as_secs_f64() * 1000.0;
        eprintln!("[feynobg trace={trace_id} event=inference_done] ms={inf_ms:.0}");

        // Explicitly free the preprocessed tensor data before postprocessing
        // (chw_input was moved into tensor, but let's be explicit for clarity)

        // --- 4. Postprocess ---
        let t_post = Instant::now();

        // Resize alpha mask back to original image dimensions
        let mask_resized = bilinear_resize(
            &mask_1024,
            INPUT_SIZE as u32,
            INPUT_SIZE as u32,
            orig_w,
            orig_h,
        );
        drop(mask_1024); // free the 1024×1024 float buffer ASAP

        // Create transparent PNG (RGBA)
        let mut out_img = image::RgbaImage::new(orig_w, orig_h);
        for y in 0..orig_h {
            for x in 0..orig_w {
                let idx = (y * orig_w + x) as usize;
                let orig_pixel = rgb8.get_pixel(x, y);
                let alpha_val = (mask_resized[idx] * 255.0).clamp(0.0, 255.0).round() as u8;

                out_img.put_pixel(
                    x,
                    y,
                    image::Rgba([orig_pixel[0], orig_pixel[1], orig_pixel[2], alpha_val]),
                );
            }
        }
        drop(mask_resized);
        drop(rgb8);

        eprintln!(
            "[feynobg trace={trace_id} event=postprocess_done] ms={:.0}",
            t_post.elapsed().as_secs_f64() * 1000.0
        );

        // --- 5. Save output PNG ---
        out_img
            .save(&output_path_buf)
            .map_err(|e| format!("save output: {e}"))?;
        eprintln!(
            "[feynobg trace={trace_id} event=output_saved] path={}",
            output_path_buf.display()
        );

        Ok(())
    })
    .await;

    // Release CPU cores back to CPU Broker
    crate::moebius::cpu::release_cpu(cores, trace_id).await;

    match process_res {
        Ok(Ok(())) => {
            let duration = start_time.elapsed();
            log_ev!("feynobg", trace_id, "engine_complete",
                "duration_sec" => duration.as_secs_f32());
            Ok(duration)
        }
        Ok(Err(e)) => {
            log_ev!("feynobg", trace_id, "engine_failed", "err" => &e);
            Err(e)
        }
        Err(e) => {
            let msg = format!("spawn_blocking panicked: {e}");
            log_ev!("feynobg", trace_id, "engine_failed", "err" => &msg);
            Err(msg)
        }
    }
}
