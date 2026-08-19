#![allow(
    clippy::too_many_arguments,
    clippy::manual_div_ceil,
    clippy::type_complexity,
    clippy::needless_lifetimes,
    clippy::explicit_auto_deref,
    clippy::items_after_test_module,
    clippy::needless_pub_self,
    clippy::collapsible_if,
    clippy::to_string_in_format_args,
    clippy::unnecessary_sort_by,
    clippy::identity_op,
    clippy::manual_ok_err,
    clippy::derivable_impls,
    clippy::len_without_is_empty,
    clippy::single_match,
    clippy::redundant_closure,
    clippy::unnecessary_map_or,
    clippy::ptr_arg,
    clippy::explicit_counter_loop,
    clippy::needless_borrow,
    clippy::if_same_then_else,
    clippy::unnecessary_cast,
    clippy::needless_borrows_for_generic_args,
    clippy::double_ended_iterator_last,
    clippy::useless_format,
    clippy::manual_flatten,
    clippy::match_wildcard_for_single_variants,
    clippy::manual_map
)]

#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[macro_use]
mod log;
mod admin;
mod app;
mod bot;
pub mod common;
mod config;
mod cookie_pool;
mod database;
mod denoise;
mod deoldify;
mod emoji;
mod feynobg;
mod filecompress;
mod force_join;
mod gemini_watermark;
pub mod http;
mod i18n;
mod ip_lookup;
mod modules;
mod moebius;
mod moss_tts;
mod musicset;
mod pdfcompress;
mod pkgconvert;
mod rank;
mod redeem;
mod referral;
mod separation;
mod soundcloud;
mod spotify;
mod stats;
mod stt;
mod studio;
mod surge_dl;
mod upscale;
mod youtube;

mod error;
mod health;
mod metrics;
mod sync_util;
pub mod validation;

#[cfg(feature = "testapi")]
mod testapi;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    #[cfg(feature = "testapi")]
    if std::env::var("TESTAPI_ENABLED").unwrap_or_default() == "1" {
        return testapi::run().await;
    }
    app::run().await
}
