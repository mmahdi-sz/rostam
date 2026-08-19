//! Shared CPU Broker client and RAII core lease management.
//!
//! Hosts the `CpuBrokerGuard` which manages acquiring, thread pinning, and releasing
//! cores leased from the separation-service CPU Broker (`http://127.0.0.1:6589`).

use std::time::Duration;

const SEP_BASE: &str = "http://127.0.0.1:6589";

/// An active lease on CPU cores granted by the central separation service broker.
///
/// Handles automatic core release and memory trimming on exit.
#[derive(Debug)]
pub struct CpuBrokerGuard {
    cores: Vec<i32>,
    trace_id: u64,
    domain: &'static str,
    released: bool,
}

impl CpuBrokerGuard {
    /// Attempts to acquire CPU cores from the broker service (`http://127.0.0.1:6589`).
    ///
    /// Retries up to 3 times with exponential backoff. Returns a guard which may have
    /// empty cores if acquisition ultimately failed.
    pub async fn acquire(user_id: i64, trace_id: u64, domain: &'static str) -> Self {
        let cores = crate::moebius::cpu::acquire_cpu(user_id, trace_id).await;
        Self {
            cores,
            trace_id,
            domain,
            released: false,
        }
    }

    /// Constructs a `CpuBrokerGuard` from already-acquired cores.
    pub fn from_cores(cores: Vec<i32>, trace_id: u64, domain: &'static str) -> Self {
        Self {
            cores,
            trace_id,
            domain,
            released: false,
        }
    }

    /// Checks whether the user already has an active CPU-bound job running globally.
    ///
    /// Anti-spam guard: MUST be checked before queueing to prevent multi-job flooding.
    pub async fn is_user_busy(user_id: i64) -> bool {
        crate::moebius::cpu::is_user_cpu_busy(user_id).await
    }

    /// Returns the slice of core indices assigned to this lease.
    pub fn cores(&self) -> &[i32] {
        &self.cores
    }

    /// Returns true if cores were successfully allocated.
    pub fn is_acquired(&self) -> bool {
        !self.cores.is_empty()
    }

    /// Pins the calling OS thread to the acquired cores.
    ///
    /// MUST be called from *inside* a `spawn_blocking` closure or worker thread,
    /// not from the async task context.
    pub fn pin_current_thread(&self) {
        if !self.cores.is_empty() {
            crate::moebius::cpu::pin_current_thread(&self.cores, self.trace_id);
        }
    }

    /// Explicit async release of CPU cores back to the broker and trims host memory.
    ///
    /// Prefer calling this explicitly at the end of execution to observe release telemetry.
    pub async fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let cores = std::mem::take(&mut self.cores);
        if !cores.is_empty() {
            crate::moebius::cpu::release_cpu(cores, self.trace_id).await;
        } else {
            crate::moebius::cpu::trim_memory();
        }
    }

    /// Disarms the guard so `Drop` does not trigger a background release.
    pub fn disarm(&mut self) -> Vec<i32> {
        self.released = true;
        std::mem::take(&mut self.cores)
    }
}

impl Drop for CpuBrokerGuard {
    fn drop(&mut self) {
        // Job finished / dropped — return freed heap pages back to the kernel immediately.
        crate::moebius::cpu::trim_memory();

        if self.released || self.cores.is_empty() {
            return;
        }
        self.released = true;
        let cores = std::mem::take(&mut self.cores);
        let trace_id = self.trace_id;
        let domain = self.domain;

        crate::log::emit(
            domain,
            trace_id,
            "cpu_guard_dropped_auto_release",
            &format!("cores={cores:?}"),
        );

        tokio::spawn(async move {
            let client = crate::http::client();
            let body = serde_json::json!({ "cores": cores });
            let _ = client
                .post(format!("{SEP_BASE}/cpu/release"))
                .json(&body)
                .timeout(Duration::from_secs(10))
                .send()
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_broker_guard_creation_and_disarm() {
        let mut guard = CpuBrokerGuard::from_cores(vec![0, 1], 12345, "test");
        assert!(guard.is_acquired());
        assert_eq!(guard.cores(), &[0, 1]);

        let disarmed = guard.disarm();
        assert_eq!(disarmed, vec![0, 1]);
        assert!(!guard.is_acquired());
        assert!(guard.cores().is_empty());
    }

    #[tokio::test]
    async fn test_cpu_broker_guard_explicit_release() {
        let mut guard = CpuBrokerGuard::from_cores(vec![], 12345, "test");
        assert!(!guard.is_acquired());
        guard.release().await;
        assert!(guard.released);
    }
}
