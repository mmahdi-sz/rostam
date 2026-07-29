use std::net::IpAddr;

pub fn sanitize_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.len() > 2048 {
        return None;
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return None;
    }
    Some(url.to_string())
}

pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
        .take(255)
        .collect()
}

pub fn sanitize_text_input(text: &str, max_len: usize) -> String {
    text.chars().take(max_len).collect()
}

/// SSRF Prevention: Checks if a URL is safe to download from (not pointing to internal/private IPs).
pub fn is_safe_url(url: &str) -> bool {
    let Some(sanitized) = sanitize_url(url) else {
        return false;
    };

    // Extract host part
    let host = match sanitized.split("://").nth(1) {
        Some(rest) => {
            let host_port = rest.split('/').next().unwrap_or("");
            host_port
                .split(':')
                .next()
                .unwrap_or("")
                .trim_matches(&['[', ']'][..])
        }
        None => return false,
    };

    if host.is_empty() {
        return false;
    }

    // Check host strings like localhost or local domain
    let host_lower = host.to_lowercase();
    if host_lower == "localhost"
        || host_lower.ends_with(".local")
        || host_lower.ends_with(".internal")
    {
        return false;
    }

    // If host is a direct IP address, parse and check private ranges
    if let Ok(ip) = host.parse::<IpAddr>() {
        return is_public_ip(ip);
    }

    true
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // Loopback (127.0.0.0/8)
            if octets[0] == 127 {
                return false;
            }
            // Private 10.0.0.0/8
            if octets[0] == 10 {
                return false;
            }
            // Private 172.16.0.0/12
            if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                return false;
            }
            // Private 192.168.0.0/16
            if octets[0] == 192 && octets[1] == 168 {
                return false;
            }
            // Link-local / Cloud metadata (169.254.0.0/16)
            if octets[0] == 169 && octets[1] == 254 {
                return false;
            }
            // 0.0.0.0
            if octets[0] == 0 {
                return false;
            }
            true
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return false;
            }
            let segments = v6.segments();
            // Unique local address (fc00::/7 -> fc00 to fdff)
            if (segments[0] & 0xfe00) == 0xfc00 {
                return false;
            }
            // Link local (fe80::/10)
            if (segments[0] & 0xffc0) == 0xfe80 {
                return false;
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_url() {
        assert_eq!(
            sanitize_url("  https://example.com/file  "),
            Some("https://example.com/file".to_string())
        );
        assert_eq!(sanitize_url("ftp://example.com"), None);
        assert_eq!(sanitize_url("javascript:alert(1)"), None);
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "....etcpasswd");
        assert_eq!(
            sanitize_filename("valid_file-name.mp4"),
            "valid_file-name.mp4"
        );
    }

    #[test]
    fn test_sanitize_text_input() {
        let long_str = "a".repeat(300);
        assert_eq!(sanitize_text_input(&long_str, 10).len(), 10);
    }

    #[test]
    fn test_is_safe_url() {
        assert!(is_safe_url("https://youtube.com/watch?v=12345"));
        assert!(!is_safe_url("http://127.0.0.1/admin"));
        assert!(!is_safe_url("http://localhost:8080/secret"));
        assert!(!is_safe_url("http://10.0.0.1/internal"));
        assert!(!is_safe_url("http://192.168.1.1/router"));
        assert!(!is_safe_url("http://169.254.169.254/latest/meta-data/"));
    }
}
