//! CPU broker client + thread pinning for the in-process Moebius pipeline.
//! Same broker endpoints `upscale::handle` uses (hosted by `separation-service`
//! on :6589) — the broker just reserves core indices via Redis, it doesn't
//! care whether the caller runs a subprocess or, as here, pins its own
//! blocking-task thread before running ONNX inference on it.

use std::sync::OnceLock;
use std::time::Duration;

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

const SEP_BASE: &str = "http://127.0.0.1:6589";

pub async fn acquire_cpu(user_id: i64, trace_id: u64) -> Vec<i32> {
    let client = HTTP_CLIENT.get_or_init(reqwest::Client::new);
    let res = client
        .post(format!("{SEP_BASE}/cpu/acquire"))
        .form(&[
            ("user_id", user_id.to_string()),
            ("is_vip", "false".to_string()),
        ])
        .timeout(Duration::from_secs(120))
        .send()
        .await;
    match res {
        Ok(r) => {
            let json: serde_json::Value = r.json().await.unwrap_or_default();
            let cores: Vec<i32> = json
                .get("cores")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            log_ev!("gwm", trace_id, "cpu_acquired", "cores" => format!("{cores:?}"));
            cores
        }
        Err(e) => {
            log_ev!("gwm", trace_id, "cpu_acquire_failed", "=>" => format!("fail err={e}"));
            vec![]
        }
    }
}

/// Hands freed heap pages back to the kernel. glibc parks them in per-thread
/// arenas instead, so after one big job (Vosk model, whole-file buffer) RSS
/// stays at the high-water mark through hours of idle. Only walks the free
/// lists — safe to call on every job exit.
pub fn trim_memory() {
    #[cfg(target_env = "gnu")]
    unsafe {
        libc::malloc_trim(0);
    }
}

pub async fn release_cpu(cores: Vec<i32>, trace_id: u64) {
    // Job finished — return the pages before going idle.
    trim_memory();
    if cores.is_empty() {
        return;
    }
    let client = HTTP_CLIENT.get_or_init(reqwest::Client::new);
    let body = serde_json::json!({ "cores": cores });
    let r = client
        .post(format!("{SEP_BASE}/cpu/release"))
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    log_ev!("gwm", trace_id, "cpu_released", "cores" => format!("{cores:?}"), "=>" => if r.is_ok() { "ok" } else { "fail" });
}

/// Pin the *calling* OS thread to `cores`. Must be called from inside the
/// `spawn_blocking` task that will actually run inference — pinning any
/// other thread (e.g. the async task that awaited it) has no effect here.
pub fn pin_current_thread(cores: &[i32], trace_id: u64) {
    if cores.is_empty() {
        return;
    }
    #[cfg(target_os = "linux")]
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for &c in cores {
            if c >= 0 && (c as usize) < libc::CPU_SETSIZE as usize {
                libc::CPU_SET(c as usize, &mut set);
            }
        }
        // pid=0 => calling thread (Linux-specific semantics of sched_setaffinity).
        let ret = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
        log_ev!("gwm", trace_id, "pin_affinity", "cores" => format!("{cores:?}"), "ret" => ret);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (cores, trace_id);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_trim_memory_is_callable() {
        // Smoke test: the FFI call must not abort under the real allocator.
        let big: Vec<u8> = vec![7u8; 8 << 20];
        drop(big);
        super::trim_memory();
    }
}
