use std::time::Duration;

use serde_json::Value;

use super::types::IpReport;

const TIMEOUT: Duration = Duration::from_secs(5);
const USER_AGENT: &str = "ros-telegram-bot-ip-lookup/1.0";

pub async fn fetch_all(ip: &str, trace_id: u64) -> IpReport {
    let (rdap, ip_api, ipinfo, bgpview, abuseipdb) = tokio::join!(
        fetch_rdap(ip),
        fetch_ip_api(ip),
        fetch_ipinfo(ip),
        fetch_bgpview(ip),
        fetch_abuseipdb(ip),
    );

    let mut report = IpReport {
        ip: ip.to_string(),
        ..Default::default()
    };

    match rdap {
        Some(v) => apply_rdap(&mut report, &v),
        None => log_ev!("ip_lookup", trace_id, "source_rdap", "=>" => "fail"),
    }
    match ip_api {
        Some(v) => apply_ip_api(&mut report, &v),
        None => log_ev!("ip_lookup", trace_id, "source_ip_api", "=>" => "fail"),
    }
    match ipinfo {
        Some(v) => apply_ipinfo(&mut report, &v),
        None => log_ev!("ip_lookup", trace_id, "source_ipinfo", "=>" => "skip_or_fail"),
    }
    match bgpview {
        Some(v) => {
            log_ev!("ip_lookup", trace_id, "source_bgpview", "=>" => "ok",
                "other_prefixes" => v.get("other_prefixes").and_then(|p| p.as_array()).map(|a| a.len()).unwrap_or(0));
            apply_bgpview(&mut report, &v);
        }
        None => log_ev!("ip_lookup", trace_id, "source_bgpview", "=>" => "fail"),
    }
    match abuseipdb {
        Some(v) => {
            log_ev!("ip_lookup", trace_id, "source_abuseipdb", "=>" => "ok",
                "score" => v.get("abuseConfidenceScore").and_then(|x| x.as_i64()).unwrap_or(-1));
            apply_abuseipdb(&mut report, &v);
        }
        None => log_ev!("ip_lookup", trace_id, "source_abuseipdb", "=>" => "skip_or_fail"),
    }

    report
}

async fn get_json(url: &str, headers: &[(&str, &str)]) -> Option<Value> {
    if !crate::validation::is_safe_url(url) {
        eprintln!("[ip_lookup] SSRF block: rejected url={url}");
        return None;
    }
    let client = reqwest::Client::new();
    let mut req = client
        .get(url)
        .timeout(TIMEOUT)
        .header("User-Agent", USER_AGENT);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = tokio::time::timeout(TIMEOUT, req.send()).await.ok()?.ok()?;
    resp.json::<Value>().await.ok()
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

fn f(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

async fn fetch_rdap(ip: &str) -> Option<Value> {
    get_json(&format!("https://rdap.org/ip/{ip}"), &[]).await
}

/// Extracts a specified field (email/tel/fn) from an entity's vcardArray.
fn vcard_field(entity: &Value, field: &str) -> Option<String> {
    entity
        .get("vcardArray")
        .and_then(|vc| vc.as_array())
        .and_then(|vc| vc.get(1))
        .and_then(|fields| fields.as_array())
        .and_then(|fields| {
            fields
                .iter()
                .find(|f| f.get(0).and_then(|n| n.as_str()) == Some(field))
        })
        .and_then(|entry| entry.get(3))
        .and_then(|v| v.as_str())
        .map(|s| s.trim_start_matches("tel:").to_string())
}

fn find_entity_by_role<'a>(v: &'a Value, role: &str) -> Option<&'a Value> {
    v.get("entities")
        .and_then(|e| e.as_array())
        .and_then(|arr| {
            arr.iter().find(|ent| {
                ent.get("roles")
                    .and_then(|r| r.as_array())
                    .map(|roles| roles.iter().any(|r| r.as_str() == Some(role)))
                    .unwrap_or(false)
            })
        })
}

fn rdap_event(v: &Value, action: &str) -> Option<String> {
    v.get("events")
        .and_then(|e| e.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|ev| ev.get("eventAction").and_then(|a| a.as_str()) == Some(action))
        })
        .and_then(|ev| s(ev, "eventDate"))
}

fn apply_rdap(report: &mut IpReport, v: &Value) {
    report.rdap_netrange = match (s(v, "startAddress"), s(v, "endAddress")) {
        (Some(a), Some(b)) => Some(format!("{a} - {b}")),
        _ => None,
    };
    report.rdap_network_name = s(v, "name");
    report.rdap_parent_handle = s(v, "parentHandle");
    report.rdap_country = s(v, "country");
    report.rdap_registered = rdap_event(v, "registration");
    report.rdap_updated = rdap_event(v, "last changed");

    let registrant = find_entity_by_role(v, "registrant").or_else(|| {
        v.get("entities")
            .and_then(|e| e.as_array())
            .and_then(|a| a.first())
    });
    report.rdap_org = registrant
        .and_then(|ent| vcard_field(ent, "fn"))
        .or_else(|| s(v, "name"));

    if let Some(abuse) = find_entity_by_role(v, "abuse") {
        report.rdap_abuse_email = vcard_field(abuse, "email");
        report.rdap_abuse_phone = vcard_field(abuse, "tel");
    }
}

async fn fetch_ip_api(ip: &str) -> Option<Value> {
    let fields = "status,country,countryCode,region,regionName,city,zip,lat,lon,timezone,isp,org,as,asname,mobile,proxy,hosting,query";
    let v = get_json(&format!("http://ip-api.com/json/{ip}?fields={fields}"), &[]).await?;
    if v.get("status").and_then(|s| s.as_str()) != Some("success") {
        return None;
    }
    Some(v)
}

fn apply_ip_api(report: &mut IpReport, v: &Value) {
    report.geo_country = s(v, "country");
    report.geo_country_code = s(v, "countryCode");
    report.geo_region = s(v, "regionName");
    report.geo_city = s(v, "city");
    report.geo_zip = s(v, "zip");
    report.geo_lat = f(v, "lat");
    report.geo_lon = f(v, "lon");
    report.geo_timezone = s(v, "timezone");
    report.geo_isp = s(v, "isp");
    report.geo_org = s(v, "org");
    report.geo_as = s(v, "as");
    report.geo_asname = s(v, "asname");
    report.geo_mobile = v.get("mobile").and_then(|x| x.as_bool());
    report.geo_proxy = v.get("proxy").and_then(|x| x.as_bool());
    report.geo_hosting = v.get("hosting").and_then(|x| x.as_bool());
}

async fn fetch_ipinfo(ip: &str) -> Option<Value> {
    let token = crate::config::ipinfo_token()?;
    get_json(&format!("https://ipinfo.io/{ip}/json?token={token}"), &[]).await
}

fn apply_ipinfo(report: &mut IpReport, v: &Value) {
    report.ipinfo_hostname = s(v, "hostname");
    report.ipinfo_org = s(v, "org");
    report.ipinfo_city = s(v, "city");
    report.ipinfo_region = s(v, "region");
    report.ipinfo_postal = s(v, "postal");
    report.ipinfo_timezone = s(v, "timezone");
    if let Some(loc) = s(v, "loc") {
        if let Some((lat, lon)) = loc.split_once(',') {
            report.ipinfo_lat = lat.trim().parse().ok();
            report.ipinfo_lon = lon.trim().parse().ok();
        }
    }
    report.ipinfo_anycast = v.get("anycast").and_then(|x| x.as_bool());
    if let Some(privacy) = v.get("privacy") {
        report.ipinfo_vpn = privacy.get("vpn").and_then(|x| x.as_bool());
        report.ipinfo_tor = privacy.get("tor").and_then(|x| x.as_bool());
        report.ipinfo_relay = privacy.get("relay").and_then(|x| x.as_bool());
    }
}

/// bgpview.io archived (shutdown Nov 2025) — ipctl.io is compatible replacement:
/// `/v1/ip/{ip}` for current IP's ASN/prefix, `/v1/asn/{asn}` for other prefixes of same ASN.
async fn fetch_bgpview(ip: &str) -> Option<Value> {
    let ip_resp = get_json(&format!("https://api.ipctl.io/v1/ip/{ip}"), &[]).await?;
    let data = ip_resp.get("data")?.clone();
    let asn_num = data
        .get("asn")
        .and_then(|a| a.get("asn"))
        .and_then(|x| x.as_i64());

    let other_prefixes = if let Some(asn) = asn_num {
        get_json(&format!("https://api.ipctl.io/v1/asn/{asn}"), &[])
            .await
            .and_then(|v| v.get("data").and_then(|d| d.get("prefixes")).cloned())
            .unwrap_or(Value::Array(vec![]))
    } else {
        Value::Array(vec![])
    };

    Some(serde_json::json!({
        "prefix": data.get("prefix"),
        "asn": data.get("asn"),
        "other_prefixes": other_prefixes,
    }))
}

fn apply_bgpview(report: &mut IpReport, v: &Value) {
    if let Some(prefix) = v.get("prefix") {
        report.bgp_prefix = s(prefix, "prefix");
        report.bgp_country = s(prefix, "country_code");
    }
    if let Some(asn) = v.get("asn") {
        report.bgp_asn = asn.get("asn").and_then(|x| x.as_i64());
        report.bgp_asn_name = s(asn, "name");
        if report.bgp_country.is_none() {
            report.bgp_country = s(asn, "country_code");
        }
    }
    if let Some(others) = v.get("other_prefixes").and_then(|p| p.as_array()) {
        report.bgp_other_prefixes = others
            .iter()
            .filter_map(|p| s(p, "prefix").map(|prefix| (prefix, s(p, "country_code"))))
            .filter(|(prefix, _)| Some(prefix.as_str()) != report.bgp_prefix.as_deref())
            .take(5)
            .collect();
    }
}

async fn fetch_abuseipdb(ip: &str) -> Option<Value> {
    let key = crate::config::abuseipdb_key()?;
    let v = get_json(
        &format!("https://api.abuseipdb.com/api/v2/check?ipAddress={ip}&maxAgeInDays=90"),
        &[("Key", &key), ("Accept", "application/json")],
    )
    .await?;
    v.get("data").cloned()
}

fn apply_abuseipdb(report: &mut IpReport, v: &Value) {
    report.abuse_score = v.get("abuseConfidenceScore").and_then(|x| x.as_i64());
    report.abuse_reports = v.get("totalReports").and_then(|x| x.as_i64());
    report.abuse_usage_type = s(v, "usageType");
    report.abuse_is_public = v.get("isPublic").and_then(|x| x.as_bool());
    report.abuse_is_whitelisted = v.get("isWhitelisted").and_then(|x| x.as_bool());
}
