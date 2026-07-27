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
