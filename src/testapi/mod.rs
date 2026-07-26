pub mod state;
pub mod bot_mock;
pub mod endpoints;

use axum::{
    routing::post,
    Router,
};
use std::net::SocketAddr;
use crate::config;

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let api_base = config::config_value("BOT_API_BASE_URL").unwrap_or_default();
    if !api_base.contains("127.0.0.1") && !api_base.contains("localhost") {
        panic!("CRITICAL: BOT_API_BASE_URL must be a local address in test mode (got: {})", api_base);
    }

    let port_str = std::env::var("TESTAPI_PORT").unwrap_or_else(|_| "14379".to_string());
    let port: u16 = port_str.parse().expect("TESTAPI_PORT must be a valid u16");

    let app = Router::new()
        .route("/test/rank/paywall", post(endpoints::rank::test_paywall))
        // Catch-all for outgoing frankenstein calls
        .route("/bot{token}/{method}", post(bot_mock::intercept_bot_request));

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    println!("[testapi] listening on 127.0.0.1:{}", port);
    
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
    Ok(())
}
