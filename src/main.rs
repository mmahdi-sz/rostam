#[macro_use]
mod log;
mod admin;
mod app;
mod rank;
mod bot;
mod config;
mod cookie_pool;
mod database;
mod denoise;
mod emoji;
mod force_join;
mod gemini_watermark;
mod i18n;
mod ip_lookup;
mod modules;
mod moebius;
mod pdfcompress;
mod redeem;
mod referral;
mod separation;
mod stats;
mod stt;
mod surge_dl;
mod upscale;
mod youtube;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run().await
}
