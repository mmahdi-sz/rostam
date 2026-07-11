mod format;
mod handle;
mod lists;
mod sources;
mod types;

pub use handle::{
    enter_ip_lookup, handle_ip_command, handle_ip_lookup_cancel, handle_ip_lookup_text,
    CB_IP_LOOKUP_CANCEL, CB_TOOLS_IP_LOOKUP,
};
pub use lists::spawn_refresher;

/// دستی/شبکه‌محور: `cargo test ip_lookup_card_smoke -- --ignored --nocapture`
/// کارت واقعی رو برای چند آی‌پی نمونه می‌سازه تا قبل از دیپلوی چشمی چک بشه.
#[cfg(test)]
mod smoke {
    use std::net::IpAddr;

    #[tokio::test]
    #[ignore]
    async fn ip_lookup_card_smoke() {
        super::lists::refresh_once().await;
        for ip_str in ["8.8.8.8", "185.220.101.1", "1.1.1.1"] {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            let ip: IpAddr = ip_str.parse().unwrap();
            let (report, matches) = tokio::join!(
                super::sources::fetch_all(ip_str, 0),
                super::lists::classify(ip),
            );
            let card = super::format::format_report(&report, &matches);
            println!("\n===== {ip_str} =====\n{card}\n");
        }
    }
}
