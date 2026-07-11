use super::lists::ListMatches;
use super::types::IpReport;
use crate::i18n::{t, tf};

pub fn format_report(report: &IpReport, matches: &ListMatches) -> String {
    let mut sections: Vec<String> = Vec::new();

    if let Some(s) = ownership_section(report) { sections.push(s); }
    if let Some(s) = location_section(report) { sections.push(s); }
    if let Some(s) = network_section(report) { sections.push(s); }
    if let Some(s) = risk_section(report, matches) { sections.push(s); }
    if let Some(s) = abuse_contact_section(report) { sections.push(s); }

    let header = tf("ip_lookup.result_header", &[("ip", &escape_md(&report.ip))]);
    if sections.is_empty() {
        format!("{header}\n\n{}", t("ip_lookup.error.no_data"))
    } else {
        format!("{header}\n\n{}", sections.join("\n\n"))
    }
}

fn ownership_section(report: &IpReport) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(org) = report.rdap_org.as_ref().or(report.ipinfo_org.as_ref()).or(report.geo_org.as_ref()) {
        push_line(&mut lines, "ip_lookup.label.org", org);
    }
    if let Some(name) = &report.rdap_network_name {
        push_line(&mut lines, "ip_lookup.label.network_name", name);
    }
    if let Some(range) = &report.rdap_netrange {
        push_line(&mut lines, "ip_lookup.label.netrange", range);
    }
    if let Some(reg) = &report.rdap_registered {
        push_line(&mut lines, "ip_lookup.label.registered", reg);
    }
    if let Some(upd) = &report.rdap_updated {
        push_line(&mut lines, "ip_lookup.label.updated", upd);
    }
    if let Some(parent) = &report.rdap_parent_handle {
        push_line(&mut lines, "ip_lookup.label.parent_network", parent);
    }
    section(lines, "ip_lookup.section.ownership")
}

fn location_section(report: &IpReport) -> Option<String> {
    let mut lines = Vec::new();
    let country = report.geo_country.as_ref().or(report.rdap_country.as_ref());
    let code = report.geo_country_code.as_ref().or(report.bgp_country.as_ref());
    match (country, code) {
        (Some(c), Some(code)) if c != code => push_line(&mut lines, "ip_lookup.label.country", &format!("{c} ({code})")),
        (Some(c), _) => push_line(&mut lines, "ip_lookup.label.country", c),
        (None, Some(code)) => push_line(&mut lines, "ip_lookup.label.country", code),
        (None, None) => {}
    }
    if let Some(r) = report.geo_region.as_ref().or(report.ipinfo_region.as_ref()) {
        push_line(&mut lines, "ip_lookup.label.region", r);
    }
    if let Some(c) = report.geo_city.as_ref().or(report.ipinfo_city.as_ref()) {
        push_line(&mut lines, "ip_lookup.label.city", c);
    }
    if let Some(zip) = report.geo_zip.as_ref().or(report.ipinfo_postal.as_ref()) {
        push_line(&mut lines, "ip_lookup.label.postal", zip);
    }
    let latlon = report.geo_lat.zip(report.geo_lon).or_else(|| report.ipinfo_lat.zip(report.ipinfo_lon));
    if let Some((lat, lon)) = latlon {
        // MarkdownV2 link URLs only need `)`/`\` escaped, not the general punctuation set;
        // this URL never contains either, so it's passed through raw.
        let link = format!("https://maps.google.com/?q={lat},{lon}");
        lines.push(tf("ip_lookup.label.coords", &[("value", &link)]));
    }
    if let Some(tz) = report.geo_timezone.as_ref().or(report.ipinfo_timezone.as_ref()) {
        push_line(&mut lines, "ip_lookup.label.timezone", tz);
    }
    section(lines, "ip_lookup.section.location")
}

fn network_section(report: &IpReport) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(isp) = &report.geo_isp {
        push_line(&mut lines, "ip_lookup.label.isp", isp);
    }
    if let Some(org) = &report.geo_org {
        if Some(org) != report.geo_isp.as_ref() {
            push_line(&mut lines, "ip_lookup.label.net_org", org);
        }
    }
    let asn = report.bgp_asn.map(|n| n.to_string()).or_else(|| {
        report.geo_as.as_ref().and_then(|s| s.split_whitespace().next()).map(|s| s.trim_start_matches("AS").to_string())
    });
    if let Some(asn) = asn {
        push_line(&mut lines, "ip_lookup.label.asn", &asn);
    }
    if let Some(name) = report.bgp_asn_name.as_ref().or(report.geo_asname.as_ref()) {
        push_line(&mut lines, "ip_lookup.label.asn_name", name);
    }
    if let Some(prefix) = &report.bgp_prefix {
        push_line(&mut lines, "ip_lookup.label.prefix", prefix);
    }
    if let Some(host) = &report.ipinfo_hostname {
        push_line(&mut lines, "ip_lookup.label.hostname", host);
    }
    if !report.bgp_other_prefixes.is_empty() {
        let list = report.bgp_other_prefixes.iter()
            .map(|(prefix, country)| match country {
                Some(c) => format!("{prefix} ({c})"),
                None => prefix.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        push_line(&mut lines, "ip_lookup.label.other_prefixes", &list);
    }
    if report.ipinfo_anycast == Some(true) {
        lines.push(t("ip_lookup.flag.anycast"));
    }
    section(lines, "ip_lookup.section.network")
}

fn risk_section(report: &IpReport, matches: &ListMatches) -> Option<String> {
    let mut lines = Vec::new();

    push_flag(&mut lines, "ip_lookup.flag.proxy", report.geo_proxy.unwrap_or(false));
    push_flag(&mut lines, "ip_lookup.flag.hosting", report.geo_hosting.unwrap_or(false));
    push_flag(&mut lines, "ip_lookup.flag.mobile", report.geo_mobile.unwrap_or(false));
    push_flag(&mut lines, "ip_lookup.flag.vpn", report.ipinfo_vpn.unwrap_or(false));
    push_flag(&mut lines, "ip_lookup.flag.tor", matches.is_tor || report.ipinfo_tor.unwrap_or(false));
    push_flag(&mut lines, "ip_lookup.flag.relay", report.ipinfo_relay.unwrap_or(false));
    push_flag(&mut lines, "ip_lookup.flag.spamhaus", matches.in_spamhaus);

    if let Some(provider) = matches.cloud_provider {
        match &matches.cloud_region {
            Some(region) => push_line(&mut lines, "ip_lookup.label.cloud_provider", &format!("{provider} ({region})")),
            None => push_line(&mut lines, "ip_lookup.label.cloud_provider", provider),
        }
    }

    if let Some(score) = report.abuse_score {
        push_line(&mut lines, "ip_lookup.label.abuse_score", &format!("{score}%"));
    }
    if let Some(reports) = report.abuse_reports {
        push_line(&mut lines, "ip_lookup.label.abuse_reports", &reports.to_string());
    }
    if let Some(usage) = &report.abuse_usage_type {
        push_line(&mut lines, "ip_lookup.label.usage_type", usage);
    }
    if report.abuse_is_public == Some(false) {
        lines.push(t("ip_lookup.flag.abuse_private"));
    }
    if report.abuse_is_whitelisted == Some(true) {
        lines.push(t("ip_lookup.flag.abuse_whitelisted"));
    }

    section(lines, "ip_lookup.section.risk")
}

fn abuse_contact_section(report: &IpReport) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(email) = &report.rdap_abuse_email {
        push_line(&mut lines, "ip_lookup.label.abuse_email", email);
    }
    if let Some(phone) = &report.rdap_abuse_phone {
        push_line(&mut lines, "ip_lookup.label.abuse_phone", phone);
    }
    if report.abuse_score.is_some() {
        let url = format!("https://www.abuseipdb.com/check/{}", report.ip);
        lines.push(tf("ip_lookup.abuse_report_note", &[("value", &url)]));
    }
    section(lines, "ip_lookup.section.abuse_contact")
}

fn section(lines: Vec<String>, header_key: &str) -> Option<String> {
    if lines.is_empty() {
        None
    } else {
        Some(format!("{}\n{}", t(header_key), lines.join("\n")))
    }
}

fn push_line(lines: &mut Vec<String>, label_key: &str, value: &str) {
    lines.push(tf(label_key, &[("value", &escape_md(value))]));
}

fn push_flag(lines: &mut Vec<String>, key: &str, on: bool) {
    if on {
        lines.push(t(key));
    }
}

fn escape_md(s: &str) -> String {
    s.chars().map(|c| match c {
        '*' | '\\' | '_' | '[' | ']' | '(' | ')' | '~' | '`' | '>'
        | '#' | '+' | '-' | '=' | '|' | '{' | '}' | '.' | '!' => format!("\\{c}"),
        other => other.to_string(),
    }).collect()
}
