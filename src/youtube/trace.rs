// ponytail: thin shim — all youtube submodules import from here; counter is now global in crate::log.

pub fn log_trace(trace_id: u64, event: &str, details: &str) {
    crate::log::emit("yt", trace_id, event, details);
}
