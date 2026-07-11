//! Lazily-loaded ONNX Runtime sessions for the three Moebius graphs. Weights
//! live outside the compiled binary (`files/models/moebius/*.onnx`, see
//! `MODEL_DIR`) and are loaded from disk on first use — swapping/updating the
//! model files never requires a rebuild, only a restart.

use std::sync::{Mutex, OnceLock};

use ort::session::{builder::GraphOptimizationLevel, Session};

const MODEL_DIR: &str = "files/models/moebius";

pub struct Sessions {
    pub encoder: Mutex<Session>,
    pub unet: Mutex<Session>,
    pub decoder: Mutex<Session>,
}

static SESSIONS: OnceLock<Result<Sessions, String>> = OnceLock::new();

fn build_session(path: &std::path::Path, threads: usize) -> Result<Session, String> {
    (|| -> ort::Result<Session> {
        Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_threads(threads)?
            .commit_from_file(path)
    })()
    .map_err(|e| format!("{path:?}: {e}"))
}

/// Get (loading on first call) the shared session set. `threads` sizes each
/// session's intra-op thread pool and is only honored on the very first
/// call — later calls with a different `threads` are ignored, matching the
/// "one model load per process lifetime" assumption used everywhere else the
/// bot loads a model. Callers should also pin the *calling* OS thread to the
/// same core set before running inference: ONNX Runtime's worker threads
/// inherit the affinity mask of the thread that spawns its thread pool (true
/// of pthread_create on Linux), which is the same trick `upscale::handle`
/// uses to pin `realesrgan-ncnn-vulkan` subprocesses to broker-reserved
/// cores.
pub fn sessions(trace_id: u64, threads: usize) -> Result<&'static Sessions, String> {
    SESSIONS
        .get_or_init(|| {
            let base = std::path::Path::new(MODEL_DIR);
            log_ev!("gwm", trace_id, "moebius_model_load_start", "dir" => MODEL_DIR, "threads" => threads);
            let t0 = std::time::Instant::now();

            let encoder = build_session(&base.join("vae_encoder.onnx"), threads)?;
            let unet = build_session(&base.join("unet.onnx"), threads)?;
            let decoder = build_session(&base.join("vae_decoder.onnx"), threads)?;

            log_ev!("gwm", trace_id, "moebius_model_load_done",
                "elapsed" => format!("{:.2}s", t0.elapsed().as_secs_f64()));
            Ok(Sessions { encoder: Mutex::new(encoder), unet: Mutex::new(unet), decoder: Mutex::new(decoder) })
        })
        .as_ref()
        .map_err(|e| e.clone())
}
