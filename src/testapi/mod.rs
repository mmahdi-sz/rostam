pub mod bot_mock;
pub mod endpoints;
pub mod state;

use crate::config;
use axum::{Router, routing::post};
use std::net::SocketAddr;

pub async fn run() -> anyhow::Result<()> {
    let api_base = config::config_value("BOT_API_BASE_URL").unwrap_or_default();
    if !api_base.contains("127.0.0.1") && !api_base.contains("localhost") {
        panic!("CRITICAL: BOT_API_BASE_URL must be a local address in test mode (got: {api_base})");
    }

    let port_str = std::env::var("TESTAPI_PORT").unwrap_or_else(|_| "14379".to_string());
    let port: u16 = port_str.parse().expect("TESTAPI_PORT must be a valid u16");

    let app = Router::new()
        .route("/test/rank/paywall", post(endpoints::rank::test_paywall))
        .route("/test/rank/panel", post(endpoints::rank::test_rank_panel))
        .route(
            "/test/rank/free_rank",
            post(endpoints::rank::test_free_rank),
        )
        .route(
            "/test/emoji/premium_render",
            post(endpoints::emoji::test_premium_render),
        )
        .route(
            "/test/router/callback",
            post(endpoints::router::test_callback),
        )
        .route(
            "/test/youtube/format",
            post(endpoints::youtube::test_youtube_format),
        )
        .route(
            "/test/youtube/quality_select",
            post(endpoints::youtube::test_youtube_quality_select),
        )
        .route(
            "/test/youtube/cancel",
            post(endpoints::youtube::test_youtube_cancel),
        )
        .route(
            "/test/pdfcompress/menu",
            post(endpoints::pdfcompress::test_pdf_compress),
        )
        .route(
            "/test/compress/submit",
            post(endpoints::compress::test_filecompress),
        )
        .route(
            "/test/compress/ux",
            post(endpoints::compress::test_filecompress_ux),
        )
        .route(
            "/test/stt/recognize",
            post(endpoints::ai::test_stt_recognize),
        )
        .route(
            "/test/separation/submit",
            post(endpoints::ai::test_separation_submit),
        )
        .route(
            "/test/emoji/panel",
            post(endpoints::emoji::test_emoji_panel),
        )
        .route(
            "/test/start/guide",
            post(endpoints::guide::test_start_guide),
        )
        .route("/test/quota", post(endpoints::quota::test_quota))
        .route("/test/gwm/detect", post(endpoints::ai::test_gwm_detect))
        .route(
            "/test/denoise/process",
            post(endpoints::ai::test_denoise_process),
        )
        .route("/test/tts/generate", post(endpoints::ai::test_tts_generate))
        .route("/test/tts/ux", post(endpoints::ai::test_tts_ux))
        .route("/test/stt/ready", post(endpoints::ai::test_stt_ready))
        .route(
            "/test/deoldify/colorized",
            post(endpoints::ai::test_deoldify_colorize),
        )
        .route("/test/nobg/process", post(endpoints::ai::test_nobg_process))
        .route(
            "/test/admin/panel",
            post(endpoints::admin::test_admin_panel),
        )
        .route(
            "/test/admin/stats_section",
            post(endpoints::admin::test_admin_stats_section),
        )
        .route(
            "/test/admin/broadcast",
            post(endpoints::admin::test_admin_broadcast),
        )
        .route(
            "/test/studio/trim",
            post(endpoints::studio::test_studio_trim),
        )
        .route(
            "/test/studio/compress",
            post(endpoints::studio::test_studio_compress),
        )
        .route(
            "/test/surge/validate_url",
            post(endpoints::surge::test_surge_validate_url),
        )
        .route(
            "/test/health/deep",
            post(endpoints::health::test_deep_health),
        )
        .route(
            "/test/redeem/apply",
            post(endpoints::redeem::test_redeem_apply),
        )
        .route(
            "/test/referral/spend",
            post(endpoints::referral::test_referral_spend),
        )
        .route(
            "/test/referral/leaderboard",
            post(endpoints::referral::test_referral_leaderboard),
        )
        .route(
            "/test/sp/download_track",
            post(endpoints::spotify::test_spotify_download_track),
        )
        .route(
            "/test/sp/cancel",
            post(endpoints::spotify::test_spotify_cancel),
        )
        .route("/test/ms/offer", post(endpoints::musicset::test_ms_offer))
        .route("/test/ms/mode", post(endpoints::musicset::test_ms_mode))
        .route(
            "/test/sc/download_track",
            post(endpoints::soundcloud::test_soundcloud_download_track),
        )
        .route(
            "/test/sc/cancel",
            post(endpoints::soundcloud::test_soundcloud_cancel),
        )
        .route(
            "/test/transfer/meter",
            post(endpoints::transfer::test_transfer_meter),
        )
        .route(
            "/test/transfer/upload",
            post(endpoints::transfer::test_transfer_upload),
        )
        // Catch-all for outgoing frankenstein calls
        .route(
            "/bot{token}/{method}",
            post(bot_mock::intercept_bot_request),
        );

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    println!("[testapi] listening on 127.0.0.1:{port}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
