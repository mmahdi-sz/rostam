use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::RwLock;

#[derive(Debug, Default, Clone)]
struct CachedLists {
    aws: Vec<(IpAddr, u8, Option<String>)>,
    gcp: Vec<(IpAddr, u8)>,
    cloudflare: Vec<(IpAddr, u8)>,
    tor_exits: HashSet<IpAddr>,
    spamhaus: Vec<(IpAddr, u8)>,
}

#[derive(Debug, Default, Clone)]
pub struct ListMatches {
    pub cloud_provider: Option<&'static str>,
    pub cloud_region: Option<String>,
    pub is_tor: bool,
    pub in_spamhaus: bool,
}

static LISTS: OnceLock<RwLock<CachedLists>> = OnceLock::new();

fn lists() -> &'static RwLock<CachedLists> {
    LISTS.get_or_init(|| RwLock::new(CachedLists::default()))
}

pub fn spawn_refresher() {
    tokio::spawn(async {
        let interval_secs = crate::config::ip_lists_refresh_secs();
        loop {
            refresh_once().await;
            tokio::time::sleep(Duration::from_secs(interval_secs)).await;
        }
    });
}

pub(super) async fn refresh_once() {
    let fresh = build_lists().await;
    let mut guard = lists().write().await;
    *guard = fresh;
    eprintln!(
        "[ip_lookup event=lists_refreshed] aws={} gcp={} cloudflare={} tor={} spamhaus={}",
        guard.aws.len(),
        guard.gcp.len(),
        guard.cloudflare.len(),
        guard.tor_exits.len(),
        guard.spamhaus.len(),
    );
}

async fn build_lists() -> CachedLists {
    let client = crate::http::client();
    let timeout = Duration::from_secs(30);

    let aws = fetch_aws_ranges(&client, timeout).await.unwrap_or_default();
    let gcp = fetch_gcp_ranges(&client, timeout).await.unwrap_or_default();
    let cloudflare = fetch_cloudflare_ranges(&client, timeout)
        .await
        .unwrap_or_default();
    let tor_exits = fetch_tor_exits(&client, timeout).await.unwrap_or_default();
    let spamhaus = fetch_spamhaus(&client, timeout).await.unwrap_or_default();

    CachedLists {
        aws,
        gcp,
        cloudflare,
        tor_exits,
        spamhaus,
    }
}

const USER_AGENT: &str = "ros-telegram-bot-ip-lookup/1.0";

async fn fetch_text(client: &reqwest::Client, url: &str, timeout: Duration) -> Option<String> {
    client
        .get(url)
        .timeout(timeout)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()
}

async fn fetch_json(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
) -> Option<serde_json::Value> {
    client
        .get(url)
        .timeout(timeout)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()
}

async fn fetch_aws_ranges(
    client: &reqwest::Client,
    timeout: Duration,
) -> Option<Vec<(IpAddr, u8, Option<String>)>> {
    let v = fetch_json(
        client,
        "https://ip-ranges.amazonaws.com/ip-ranges.json",
        timeout,
    )
    .await?;
    let mut out = Vec::new();
    if let Some(prefixes) = v.get("prefixes").and_then(|p| p.as_array()) {
        for p in prefixes {
            if let Some(cidr) = p.get("ip_prefix").and_then(|c| c.as_str()) {
                if let Some((net, prefix)) = parse_cidr(cidr) {
                    let region = p
                        .get("region")
                        .and_then(|r| r.as_str())
                        .map(|s| s.to_string());
                    out.push((net, prefix, region));
                }
            }
        }
    }
    if let Some(prefixes) = v.get("ipv6_prefixes").and_then(|p| p.as_array()) {
        for p in prefixes {
            if let Some(cidr) = p.get("ipv6_prefix").and_then(|c| c.as_str()) {
                if let Some((net, prefix)) = parse_cidr(cidr) {
                    let region = p
                        .get("region")
                        .and_then(|r| r.as_str())
                        .map(|s| s.to_string());
                    out.push((net, prefix, region));
                }
            }
        }
    }
    Some(out)
}

async fn fetch_gcp_ranges(
    client: &reqwest::Client,
    timeout: Duration,
) -> Option<Vec<(IpAddr, u8)>> {
    let v = fetch_json(
        client,
        "https://www.gstatic.com/ipranges/cloud.json",
        timeout,
    )
    .await?;
    let mut out = Vec::new();
    if let Some(prefixes) = v.get("prefixes").and_then(|p| p.as_array()) {
        for p in prefixes {
            let cidr = p
                .get("ipv4Prefix")
                .and_then(|c| c.as_str())
                .or_else(|| p.get("ipv6Prefix").and_then(|c| c.as_str()));
            if let Some(cidr) = cidr {
                if let Some(parsed) = parse_cidr(cidr) {
                    out.push(parsed);
                }
            }
        }
    }
    Some(out)
}

async fn fetch_cloudflare_ranges(
    client: &reqwest::Client,
    timeout: Duration,
) -> Option<Vec<(IpAddr, u8)>> {
    let v4 = fetch_text(client, "https://www.cloudflare.com/ips-v4", timeout)
        .await
        .unwrap_or_default();
    let v6 = fetch_text(client, "https://www.cloudflare.com/ips-v6", timeout)
        .await
        .unwrap_or_default();
    let out = v4
        .lines()
        .chain(v6.lines())
        .filter_map(parse_cidr)
        .collect();
    Some(out)
}

async fn fetch_tor_exits(client: &reqwest::Client, timeout: Duration) -> Option<HashSet<IpAddr>> {
    let text = fetch_text(
        client,
        "https://check.torproject.org/torbulkexitlist",
        timeout,
    )
    .await?;
    Some(
        text.lines()
            .filter_map(|l| l.trim().parse::<IpAddr>().ok())
            .collect(),
    )
}

async fn fetch_spamhaus(client: &reqwest::Client, timeout: Duration) -> Option<Vec<(IpAddr, u8)>> {
    let drop = fetch_text(client, "https://www.spamhaus.org/drop/drop.txt", timeout)
        .await
        .unwrap_or_default();
    let edrop = fetch_text(client, "https://www.spamhaus.org/drop/edrop.txt", timeout)
        .await
        .unwrap_or_default();
    let out = drop
        .lines()
        .chain(edrop.lines())
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with(';') {
                return None;
            }
            let cidr = l.split(';').next()?.trim();
            parse_cidr(cidr)
        })
        .collect();
    Some(out)
}

fn parse_cidr(s: &str) -> Option<(IpAddr, u8)> {
    let (addr, prefix) = s.split_once('/')?;
    let ip: IpAddr = addr.trim().parse().ok()?;
    let prefix: u8 = prefix.trim().parse().ok()?;
    Some((ip, prefix))
}

fn cidr_contains(network: IpAddr, prefix: u8, ip: IpAddr) -> bool {
    match (network, ip) {
        (IpAddr::V4(net), IpAddr::V4(ip)) => {
            if prefix == 0 {
                return true;
            }
            let mask = u32::MAX.checked_shl(32 - prefix as u32).unwrap_or(0);
            u32::from(net) & mask == u32::from(ip) & mask
        }
        (IpAddr::V6(net), IpAddr::V6(ip)) => {
            if prefix == 0 {
                return true;
            }
            let mask = u128::MAX.checked_shl(128 - prefix as u32).unwrap_or(0);
            u128::from(net) & mask == u128::from(ip) & mask
        }
        _ => false,
    }
}

fn any_contains(ranges: &[(IpAddr, u8)], ip: IpAddr) -> bool {
    ranges
        .iter()
        .any(|(net, prefix)| cidr_contains(*net, *prefix, ip))
}

fn find_aws_region(ranges: &[(IpAddr, u8, Option<String>)], ip: IpAddr) -> Option<Option<String>> {
    ranges
        .iter()
        .find(|(net, prefix, _)| cidr_contains(*net, *prefix, ip))
        .map(|(_, _, region)| region.clone())
}

pub async fn classify(ip: IpAddr) -> ListMatches {
    let guard = lists().read().await;
    let (cloud_provider, cloud_region) = if let Some(region) = find_aws_region(&guard.aws, ip) {
        (Some("AWS"), region)
    } else if any_contains(&guard.gcp, ip) {
        (Some("Google Cloud"), None)
    } else if any_contains(&guard.cloudflare, ip) {
        (Some("Cloudflare"), None)
    } else {
        (None, None)
    };
    ListMatches {
        cloud_provider,
        cloud_region,
        is_tor: guard.tor_exits.contains(&ip),
        in_spamhaus: any_contains(&guard.spamhaus, ip),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr_v4_contains() {
        let net: IpAddr = "10.0.0.0".parse().unwrap();
        let inside: IpAddr = "10.0.5.7".parse().unwrap();
        let outside: IpAddr = "10.1.0.0".parse().unwrap();
        assert!(cidr_contains(net, 16, inside));
        assert!(!cidr_contains(net, 16, outside));
    }

    #[test]
    fn cidr_v6_contains() {
        let net: IpAddr = "2606:4700::".parse().unwrap();
        let inside: IpAddr = "2606:4700::1".parse().unwrap();
        let outside: IpAddr = "2607:4700::1".parse().unwrap();
        assert!(cidr_contains(net, 32, inside));
        assert!(!cidr_contains(net, 32, outside));
    }

    #[test]
    fn parse_cidr_valid_and_invalid() {
        assert_eq!(
            parse_cidr("192.168.0.0/24"),
            Some(("192.168.0.0".parse().unwrap(), 24))
        );
        assert_eq!(parse_cidr("not-a-cidr"), None);
    }
}
