#[derive(Debug, Default, Clone)]
pub struct IpReport {
    pub ip: String,

    // rdap.org
    pub rdap_netrange: Option<String>,
    pub rdap_org: Option<String>,
    pub rdap_country: Option<String>,
    pub rdap_abuse_email: Option<String>,
    pub rdap_abuse_phone: Option<String>,
    pub rdap_registered: Option<String>,
    pub rdap_updated: Option<String>,
    pub rdap_network_name: Option<String>,
    pub rdap_parent_handle: Option<String>,

    // ip-api.com
    pub geo_country: Option<String>,
    pub geo_country_code: Option<String>,
    pub geo_region: Option<String>,
    pub geo_city: Option<String>,
    pub geo_zip: Option<String>,
    pub geo_lat: Option<f64>,
    pub geo_lon: Option<f64>,
    pub geo_timezone: Option<String>,
    pub geo_isp: Option<String>,
    pub geo_org: Option<String>,
    pub geo_as: Option<String>,
    pub geo_asname: Option<String>,
    pub geo_mobile: Option<bool>,
    pub geo_proxy: Option<bool>,
    pub geo_hosting: Option<bool>,

    // ipinfo.io
    pub ipinfo_hostname: Option<String>,
    pub ipinfo_org: Option<String>,
    pub ipinfo_city: Option<String>,
    pub ipinfo_region: Option<String>,
    pub ipinfo_postal: Option<String>,
    pub ipinfo_timezone: Option<String>,
    pub ipinfo_lat: Option<f64>,
    pub ipinfo_lon: Option<f64>,
    pub ipinfo_anycast: Option<bool>,
    pub ipinfo_vpn: Option<bool>,
    pub ipinfo_tor: Option<bool>,
    pub ipinfo_relay: Option<bool>,

    // ASN/prefix (ipctl.io — BGPView's replacement, see sources.rs)
    pub bgp_asn: Option<i64>,
    pub bgp_asn_name: Option<String>,
    pub bgp_prefix: Option<String>,
    pub bgp_country: Option<String>,
    pub bgp_other_prefixes: Vec<(String, Option<String>)>,

    // abuseipdb.com
    pub abuse_score: Option<i64>,
    pub abuse_reports: Option<i64>,
    pub abuse_usage_type: Option<String>,
    pub abuse_is_public: Option<bool>,
    pub abuse_is_whitelisted: Option<bool>,
}
