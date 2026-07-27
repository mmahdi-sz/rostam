use prometheus::{
    register_histogram_vec, register_int_counter_vec, register_int_gauge, HistogramVec,
    IntCounterVec, IntGauge,
};
use std::sync::OnceLock;

#[allow(dead_code)]
pub struct Metrics {
    pub requests_total: IntCounterVec,
    pub request_duration: HistogramVec,
    pub active_downloads: IntGauge,
    pub errors_total: IntCounterVec,
}

static METRICS: OnceLock<Metrics> = OnceLock::new();

pub fn init() {
    let _ = METRICS.get_or_init(|| Metrics {
        requests_total: register_int_counter_vec!(
            "bot_requests_total",
            "Total bot requests by feature and status",
            &["feature", "status"]
        )
        .expect("metric registration failed"),
        request_duration: register_histogram_vec!(
            "bot_request_duration_seconds",
            "Request processing duration",
            &["feature"]
        )
        .expect("metric registration failed"),
        active_downloads: register_int_gauge!(
            "bot_active_downloads",
            "Currently active downloads"
        )
        .expect("metric registration failed"),
        errors_total: register_int_counter_vec!(
            "bot_errors_total",
            "Total errors by feature",
            &["feature"]
        )
        .expect("metric registration failed"),
    });
}

pub fn get() -> &'static Metrics {
    init();
    METRICS.get().expect("metrics not initialized")
}
