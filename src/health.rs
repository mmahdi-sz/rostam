//! Minimal HTTP server for health checks (/health) and Prometheus metrics (/metrics).

use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

static HEALTHY: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);

pub fn mark_healthy() {
    HEALTHY.store(true, Ordering::SeqCst);
}

pub fn mark_ready() {
    READY.store(true, Ordering::SeqCst);
}

pub fn is_healthy() -> bool {
    HEALTHY.load(Ordering::SeqCst)
}

pub fn is_ready() -> bool {
    READY.load(Ordering::SeqCst)
}

/// Minimal HTTP health check server.
/// Listens on 127.0.0.1:port and serves /health.
use prometheus::Encoder;
use tokio::io::AsyncReadExt;

pub async fn serve(port: u16) {
    let addr = ("127.0.0.1", port);
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(event = "health_bind_failed", err = %e, port = port);
            return;
        }
    };

    tracing::info!(event = "health_server_started", port = port);

    loop {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let req_str = String::from_utf8_lossy(&buf[..n]);

            if req_str.contains("GET /metrics") {
                let encoder = prometheus::TextEncoder::new();
                let metric_families = prometheus::gather();
                let mut buffer = vec![];
                let _ = encoder.encode(&metric_families, &mut buffer);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    buffer.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.write_all(&buffer).await;
            } else {
                let healthy = is_healthy();
                let ready = is_ready();
                let status = if ready && healthy {
                    "200 OK"
                } else {
                    "503 Service Unavailable"
                };
                let body = serde_json::json!({
                    "healthy": healthy,
                    "ready": ready,
                })
                .to_string();

                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                );

                let _ = stream.write_all(resp.as_bytes()).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mark_healthy_and_ready() {
        mark_healthy();
        assert!(is_healthy());
        mark_ready();
        assert!(is_ready());
    }

    #[test]
    fn test_health_json_serialization() {
        let healthy = true;
        let ready = true;
        let body = serde_json::json!({
            "healthy": healthy,
            "ready": ready,
        })
        .to_string();
        assert!(body.contains("\"healthy\":true"));
        assert!(body.contains("\"ready\":true"));
    }

    #[test]
    fn test_metrics_gather() {
        let metric_families = prometheus::gather();
        let _ = metric_families;
    }
}
