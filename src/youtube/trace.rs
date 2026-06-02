use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TRACE_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_trace_id() -> u64 {
    NEXT_TRACE_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn log_trace(trace_id: u64, event: &str, details: &str) {
    eprintln!("[youtube trace={trace_id} event={event}] {details}");
}
