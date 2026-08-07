use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ort::session::{Session, builder::GraphOptimizationLevel};
use ort::value::Tensor;

const MODEL_PATH: &str = "files/models/deoldify/ddcolor_modelscope.onnx";
const INPUT_SIZE: usize = 512;

/// How long after the last inference to keep the session alive before unloading.
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

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

    fn get_or_load(&mut self, threads: usize) -> Result<&mut Session, String> {
        if self.session.is_none() {
            eprintln!("[deoldify] Loading ONNX session");
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

    fn unload(&mut self) -> bool {
        if self.session.is_some() {
            self.session = None;
            eprintln!("[deoldify] ONNX session unloaded (idle timeout)");
            return true;
        }
        false
    }

    fn is_idle(&self) -> bool {
        self.session.is_some() && self.last_used.elapsed() >= SESSION_IDLE_TIMEOUT
    }
}

static SESSION_HOLDER: std::sync::OnceLock<Arc<Mutex<SessionHolder>>> = std::sync::OnceLock::new();

fn session_holder() -> &'static Arc<Mutex<SessionHolder>> {
    SESSION_HOLDER.get_or_init(|| Arc::new(Mutex::new(SessionHolder::new())))
}

/// Spawn a background task that unloads the session after it has been idle
/// for SESSION_IDLE_TIMEOUT, then calls malloc_trim to return pages to the OS.
pub fn spawn_session_reaper() {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
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
                #[cfg(target_os = "linux")]
                unsafe {
                    libc::malloc_trim(0);
                }
                eprintln!("[deoldify] malloc_trim called after session unload");
            }
        }
    });
}

// ----------------------------------------------------------------------------
// CIELAB Color Math (D65 Illuminant, 2° Standard Observer)
// ----------------------------------------------------------------------------

fn srgb_to_linear(v: f32) -> f32 {
    if v > 0.04045 {
        ((v + 0.055) / 1.055).powf(2.4)
    } else {
        v / 12.92
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v > 0.0031308 {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    } else {
        12.92 * v
    }
}

fn rgb_to_xyz(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let r = srgb_to_linear(r);
    let g = srgb_to_linear(g);
    let b = srgb_to_linear(b);
    let x = r * 0.412453 + g * 0.357580 + b * 0.180423;
    let y = r * 0.212671 + g * 0.715160 + b * 0.072169;
    let z = r * 0.019334 + g * 0.119193 + b * 0.950227;
    (x, y, z)
}

fn xyz_to_lab(x: f32, y: f32, z: f32) -> (f32, f32, f32) {
    const XN: f32 = 0.950456;
    const YN: f32 = 1.000000;
    const ZN: f32 = 1.088754;

    fn f(t: f32) -> f32 {
        if t > 0.008856 {
            t.powf(1.0 / 3.0)
        } else {
            7.787 * t + 16.0 / 116.0
        }
    }

    let fx = f(x / XN);
    let fy = f(y / YN);
    let fz = f(z / ZN);

    let l = if (y / YN) > 0.008856 {
        116.0 * fy - 16.0
    } else {
        903.3 * (y / YN)
    };
    let a = 500.0 * (fx - fy);
    let b = 200.0 * (fy - fz);
    (l, a, b)
}

fn rgb_to_lab(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let (x, y, z) = rgb_to_xyz(r, g, b);
    xyz_to_lab(x, y, z)
}

fn lab_to_xyz(l: f32, a: f32, b: f32) -> (f32, f32, f32) {
    const XN: f32 = 0.950456;
    const YN: f32 = 1.000000;
    const ZN: f32 = 1.088754;

    let fy = (l + 16.0) / 116.0;
    let fx = a / 500.0 + fy;
    let fz = fy - b / 200.0;

    fn inv_f(t: f32) -> f32 {
        if t.powi(3) > 0.008856 {
            t.powi(3)
        } else {
            (t - 16.0 / 116.0) / 7.787
        }
    }

    let x = inv_f(fx) * XN;
    let y = inv_f(fy) * YN;
    let z = inv_f(fz) * ZN;
    (x, y, z)
}

fn xyz_to_rgb(x: f32, y: f32, z: f32) -> (f32, f32, f32) {
    let r_lin = x * 3.2404542 - y * 1.5371385 - z * 0.4985314;
    let g_lin = -x * 0.969_266 + y * 1.8760108 + z * 0.0415560;
    let b_lin = x * 0.0556434 - y * 0.2040259 + z * 1.0572252;
    (
        linear_to_srgb(r_lin),
        linear_to_srgb(g_lin),
        linear_to_srgb(b_lin),
    )
}

fn lab_to_rgb(l: f32, a: f32, b: f32) -> (f32, f32, f32) {
    let (x, y, z) = lab_to_xyz(l, a, b);
    let (r, g, b) = xyz_to_rgb(x, y, z);
    (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0))
}

// ----------------------------------------------------------------------------
// Image resizing helper (Bilinear for float32 buffers)
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
// Pipeline implementation
// ----------------------------------------------------------------------------

pub async fn run_deoldify_colorize(
    input_path: &Path,
    output_path: &Path,
    _render_factor: u32,
    user_id: i64,
    trace_id: u64,
) -> Result<Duration, String> {
    let start_time = Instant::now();
    log_ev!("deoldify", trace_id, "engine_start",
        "input" => input_path.display(), "model" => MODEL_PATH);

    // Acquire CPU core allocation from CPU Broker
    let cores = crate::moebius::cpu::acquire_cpu(user_id, trace_id).await;
    log_ev!("deoldify", trace_id, "cpu_acquired", "cores" => format!("{cores:?}"));

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
            "[deoldify trace={trace_id} event=image_loaded] w={orig_w} h={orig_h} load_ms={:.0}",
            t_load.elapsed().as_secs_f64() * 1000.0
        );

        // --- 2. Preprocess ---
        let t_pre = Instant::now();
        let rgb8 = img.to_rgb8();

        // Extract original L channel from full resolution image
        let mut orig_l_channel = Vec::with_capacity((orig_w * orig_h) as usize);
        for pixel in rgb8.pixels() {
            let r = pixel[0] as f32 / 255.0;
            let g = pixel[1] as f32 / 255.0;
            let b = pixel[2] as f32 / 255.0;
            let (l, _, _) = rgb_to_lab(r, g, b);
            orig_l_channel.push(l);
        }

        // Resize image to 512x512
        let resized = image::imageops::resize(
            &rgb8,
            INPUT_SIZE as u32,
            INPUT_SIZE as u32,
            image::imageops::FilterType::CatmullRom,
        );

        // Compute gray RGB (L, 0, 0 -> RGB) for the network input
        let mut chw_input = vec![0f32; 3 * INPUT_SIZE * INPUT_SIZE];
        let plane_size = INPUT_SIZE * INPUT_SIZE;

        for (idx, pixel) in resized.pixels().enumerate() {
            let r = pixel[0] as f32 / 255.0;
            let g = pixel[1] as f32 / 255.0;
            let b = pixel[2] as f32 / 255.0;

            let (l, _, _) = rgb_to_lab(r, g, b);
            // Reconstruct RGB from (l, 0, 0)
            let (gray_r, gray_g, gray_b) = lab_to_rgb(l, 0.0, 0.0);

            chw_input[idx] = gray_r;
            chw_input[plane_size + idx] = gray_g;
            chw_input[2 * plane_size + idx] = gray_b;
        }

        eprintln!(
            "[deoldify trace={trace_id} event=preprocess_done] ms={:.0}",
            t_pre.elapsed().as_secs_f64() * 1000.0
        );

        // --- 3. ONNX Inference ---
        let t_inf = Instant::now();

        let input_tensor = Tensor::from_array(([1usize, 3, INPUT_SIZE, INPUT_SIZE], chw_input))
            .map_err(|e| format!("tensor create: {e}"))?;

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

        // Output shape is [1, 2, 512, 512] (the a and b channels)
        let chw_output: Vec<f32> = output_data.to_vec();

        // Release lock and ORT outputs before postprocessing
        drop(outputs);
        drop(h);

        let inf_ms = t_inf.elapsed().as_secs_f64() * 1000.0;
        eprintln!("[deoldify trace={trace_id} event=inference_done] ms={inf_ms:.0}");

        // --- 4. Postprocess ---
        let t_post = Instant::now();

        let plane = INPUT_SIZE * INPUT_SIZE;
        let a_plane = &chw_output[0..plane];
        let b_plane = &chw_output[plane..2 * plane];

        let a_resized = bilinear_resize(
            a_plane,
            INPUT_SIZE as u32,
            INPUT_SIZE as u32,
            orig_w,
            orig_h,
        );
        let b_resized = bilinear_resize(
            b_plane,
            INPUT_SIZE as u32,
            INPUT_SIZE as u32,
            orig_w,
            orig_h,
        );

        let mut out_img = image::RgbImage::new(orig_w, orig_h);
        for y in 0..orig_h {
            for x in 0..orig_w {
                let idx = (y * orig_w + x) as usize;
                let l = orig_l_channel[idx];
                let a = a_resized[idx];
                let b = b_resized[idx];

                let (r_f, g_f, b_f) = lab_to_rgb(l, a, b);
                out_img.put_pixel(
                    x,
                    y,
                    image::Rgb([
                        (r_f * 255.0).round() as u8,
                        (g_f * 255.0).round() as u8,
                        (b_f * 255.0).round() as u8,
                    ]),
                );
            }
        }

        eprintln!(
            "[deoldify trace={trace_id} event=postprocess_done] ms={:.0}",
            t_post.elapsed().as_secs_f64() * 1000.0
        );

        // --- 5. Save output ---
        out_img
            .save(&output_path_buf)
            .map_err(|e| format!("save output: {e}"))?;
        eprintln!(
            "[deoldify trace={trace_id} event=output_saved] path={}",
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
            log_ev!("deoldify", trace_id, "engine_complete",
                "duration_sec" => duration.as_secs_f32());
            Ok(duration)
        }
        Ok(Err(e)) => {
            log_ev!("deoldify", trace_id, "engine_failed", "err" => &e);
            Err(e)
        }
        Err(e) => {
            let msg = format!("spawn_blocking panicked: {e}");
            log_ev!("deoldify", trace_id, "engine_failed", "err" => &msg);
            Err(msg)
        }
    }
}
