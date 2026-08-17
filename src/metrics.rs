//! Prometheus metrics registration and RAII metric guards.

use prometheus::{
    HistogramVec, IntCounterVec, IntGauge, register_histogram_vec, register_int_counter_vec,
    register_int_gauge,
};
use std::sync::OnceLock;

#[allow(dead_code)]
pub struct Metrics {
    pub requests_total: IntCounterVec,
    pub request_duration: HistogramVec,
    pub active_downloads: IntGauge,
    pub errors_total: IntCounterVec,
    pub youtube_downloads_total: IntCounterVec,
    pub stt_requests_total: IntCounterVec,
    pub pdf_compress_total: IntCounterVec,
    pub separation_requests_total: IntCounterVec,
    pub gwm_requests_total: IntCounterVec,
    pub transfer_bytes_total: IntCounterVec,
    pub transfer_speed_histogram: HistogramVec,
    pub transfer_files_total: IntCounterVec,
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
        active_downloads: register_int_gauge!("bot_active_downloads", "Currently active downloads")
            .expect("metric registration failed"),
        errors_total: register_int_counter_vec!(
            "bot_errors_total",
            "Total errors by feature",
            &["feature"]
        )
        .expect("metric registration failed"),
        youtube_downloads_total: register_int_counter_vec!(
            "bot_youtube_downloads_total",
            "YouTube downloads by quality and status",
            &["quality", "status"]
        )
        .expect("metric registration failed"),
        stt_requests_total: register_int_counter_vec!(
            "bot_stt_requests_total",
            "STT requests by model and status",
            &["model", "status"]
        )
        .expect("metric registration failed"),
        pdf_compress_total: register_int_counter_vec!(
            "bot_pdf_compress_total",
            "PDF compress requests by level and status",
            &["level", "status"]
        )
        .expect("metric registration failed"),
        separation_requests_total: register_int_counter_vec!(
            "bot_separation_requests_total",
            "Vocal separation requests by status",
            &["status"]
        )
        .expect("metric registration failed"),
        gwm_requests_total: register_int_counter_vec!(
            "bot_gwm_requests_total",
            "Gemini watermark removal requests by status",
            &["status"]
        )
        .expect("metric registration failed"),
        transfer_bytes_total: register_int_counter_vec!(
            "bot_transfer_bytes_total",
            "Total bytes transferred by direction (download/upload) and feature",
            &["direction", "feature"]
        )
        .expect("metric registration failed"),
        transfer_speed_histogram: register_histogram_vec!(
            "bot_transfer_speed_bytes_per_second",
            "Transfer speed in bytes per second by direction and feature",
            &["direction", "feature"]
        )
        .expect("metric registration failed"),
        transfer_files_total: register_int_counter_vec!(
            "bot_transfer_files_total",
            "Total files transferred by direction and feature",
            &["direction", "feature"]
        )
        .expect("metric registration failed"),
    });
}

pub fn get() -> &'static Metrics {
    init();
    METRICS.get().expect("metrics not initialized")
}

pub struct ActiveDownloadGuard;
impl ActiveDownloadGuard {
    pub fn new() -> Self {
        get().active_downloads.inc();
        Self
    }
}
impl Drop for ActiveDownloadGuard {
    fn drop(&mut self) {
        get().active_downloads.dec();
    }
}

pub struct RequestDurationGuard {
    feature: &'static str,
    start: std::time::Instant,
}
impl RequestDurationGuard {
    pub fn new(feature: &'static str) -> Self {
        Self {
            feature,
            start: std::time::Instant::now(),
        }
    }
}
impl Drop for RequestDurationGuard {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_secs_f64();
        get()
            .request_duration
            .with_label_values(&[self.feature])
            .observe(elapsed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_active_download_guard() {
        let initial = get().active_downloads.get();
        {
            let _guard = ActiveDownloadGuard::new();
            assert_eq!(get().active_downloads.get(), initial + 1);
        }
        assert_eq!(get().active_downloads.get(), initial);
    }

    #[test]
    fn test_request_duration_guard() {
        {
            let _guard = RequestDurationGuard::new("test_feature");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
