mod app;
mod bot;
mod config;
mod cookie_pool;
mod database;
mod denoise;
mod emoji;
mod gemini_watermark;
mod i18n;
mod modules;
mod separation;
mod stt;
mod upscale;
mod youtube;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run().await
}
