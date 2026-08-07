//! Lazily-loaded ONNX Runtime sessions for the three Moebius graphs. Weights
//! live outside the compiled binary (`files/models/moebius/*.onnx`, see
//! `MODEL_DIR`) and are loaded from disk on first use.
//!
//! Sessions are automatically **unloaded** after `SESSION_IDLE_TIMEOUT` of
//! inactivity (no inference call). On unload `malloc_trim(0)` is called so
//! the ~1.2 GB of model weights are returned to the OS immediately.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ort::session::{Session, builder::GraphOptimizationLevel};

const MODEL_DIR: &str = "files/models/moebius";

/// How long after the last inference to keep the sessions alive.
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Sessions {
    pub encoder: Mutex<Session>,
    pub unet: Mutex<Session>,
    pub decoder: Mutex<Session>,
}

struct SessionsHolder {
    /// `Arc` so the idle reaper can drop the holder's handle while an
    /// inference is still running: the running job owns its own clone and
    /// the weights stay mapped until it finishes. Before this was an
    /// `Option<Sessions>` handed out as `&'static` through a raw pointer, and
    /// a run longer than `SESSION_IDLE_TIMEOUT` (single-threaded ONNX takes
    /// ~6s per DDIM step, 19 steps ≈ 115s) was freed mid-loop — SIGSEGV.
    sessions: Option<Arc<Sessions>>,
    last_used: Instant,
}

impl SessionsHolder {
    fn new() -> Self {
        Self {
            sessions: None,
            last_used: Instant::now(),
        }
    }

    fn get_or_load(&mut self, trace_id: u64, threads: usize) -> Result<Arc<Sessions>, String> {
        if self.sessions.is_none() {
            let base = std::path::Path::new(MODEL_DIR);
            log_ev!("gwm", trace_id, "moebius_model_load_start", "dir" => MODEL_DIR, "threads" => threads);
            let t0 = Instant::now();

            let encoder = build_session(&base.join("vae_encoder.onnx"), threads)?;
            let unet = build_session(&base.join("unet.onnx"), threads)?;
            let decoder = build_session(&base.join("vae_decoder.onnx"), threads)?;

            log_ev!("gwm", trace_id, "moebius_model_load_done",
                "elapsed" => format!("{:.2}s", t0.elapsed().as_secs_f64()));
            self.sessions = Some(Arc::new(Sessions {
                encoder: Mutex::new(encoder),
                unet: Mutex::new(unet),
                decoder: Mutex::new(decoder),
            }));
        }
        self.last_used = Instant::now();
        self.sessions
            .clone()
            .ok_or_else(|| "moebius sessions missing right after load".to_string())
    }

    fn unload(&mut self) -> bool {
        if self.sessions.is_some() {
            self.sessions = None;
            eprintln!("[moebius] ONNX sessions unloaded (idle timeout)");
            return true;
        }
        false
    }

    fn is_idle(&self) -> bool {
        self.sessions.is_some() && self.last_used.elapsed() >= SESSION_IDLE_TIMEOUT
    }
}

static SESSIONS_HOLDER: std::sync::OnceLock<Arc<Mutex<SessionsHolder>>> =
    std::sync::OnceLock::new();

#[allow(private_interfaces)]
pub(crate) fn sessions_holder() -> &'static Arc<Mutex<SessionsHolder>> {
    SESSIONS_HOLDER.get_or_init(|| Arc::new(Mutex::new(SessionsHolder::new())))
}

/// Spawn the background reaper that unloads sessions after idle timeout.
/// Safe to call multiple times — only the first call starts the task.
pub fn spawn_session_reaper() {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let holder = sessions_holder().clone();
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
                eprintln!("[moebius] malloc_trim called after session unload");
            }
        }
    });
}

/// Test hook: drop the holder's handle as the idle reaper would.
#[cfg(test)]
pub(crate) fn force_unload() {
    let mut h = sessions_holder().lock().unwrap_or_else(|e| e.into_inner());
    h.unload();
}

fn build_session(path: &std::path::Path, threads: usize) -> Result<Session, String> {
    (|| -> ort::Result<Session> {
        Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_threads(threads)?
            .with_memory_pattern(false)?
            .commit_from_file(path)
    })()
    .map_err(|e| format!("{path:?}: {e}"))
}

/// Acquire the session set for one inference call. The returned `Arc` keeps
/// the weights alive for as long as the caller holds it, so the idle reaper
/// can unload the holder's handle mid-inference without freeing memory the
/// running job is still using.
pub(crate) fn sessions(trace_id: u64, threads: usize) -> Result<Arc<Sessions>, String> {
    let holder = sessions_holder();
    let mut h = holder.lock().unwrap_or_else(|e| e.into_inner());
    h.get_or_load(trace_id, threads)
}
