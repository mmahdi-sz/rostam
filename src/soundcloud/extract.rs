//! Extract SoundCloud track URLs from plain text messages.

/// Extract a normalized SoundCloud track URL from `text`.
///
/// Supported formats:
/// - `https://soundcloud.com/{artist}/{track}`
/// - `https://m.soundcloud.com/{artist}/{track}`
/// - `https://on.soundcloud.com/{short_code}`
/// - `soundcloud.com/{artist}/{track}`
pub fn extract_soundcloud_url(text: &str) -> Option<String> {
    for token in text.split(|c: char| c.is_whitespace()) {
        let token = token.trim_matches(|c: char| {
            matches!(
                c,
                '<' | '>' | '"' | '\'' | ',' | ';' | '!' | '?' | ')' | '(' | '[' | ']'
            )
        });

        if token.is_empty() {
            continue;
        }

        let lower = token.to_ascii_lowercase();
        let is_standard = lower.contains("soundcloud.com") && !lower.contains("on.soundcloud.com");
        let is_short = lower.contains("on.soundcloud.com");

        if !is_standard && !is_short {
            continue;
        }

        let url_str = if lower.starts_with("http://") || lower.starts_with("https://") {
            token.to_string()
        } else {
            format!("https://{token}")
        };

        if let Ok(url) = reqwest::Url::parse(&url_str) {
            let host = url.host_str().unwrap_or("").to_lowercase();
            let path = url.path();
            let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

            if host == "on.soundcloud.com" || host.ends_with(".on.soundcloud.com") {
                if !segments.is_empty() {
                    return Some(url_str);
                }
            } else if host == "soundcloud.com" || host.ends_with(".soundcloud.com") {
                if segments.len() >= 2 {
                    let first = segments[0].to_lowercase();
                    const RESERVED: &[&str] = &[
                        "discover",
                        "stream",
                        "upload",
                        "search",
                        "terms-of-use",
                        "you",
                        "mobile",
                        "settings",
                        "imprint",
                        "charts",
                        "stations",
                    ];
                    if !RESERVED.contains(&first.as_str()) {
                        return Some(url_str);
                    }
                }
            }
        }
    }

    None
}

/// Extract a SoundCloud playlist/album ("set") URL from `text`.
///
/// `extract_soundcloud_url` also matches `/{artist}/sets/{name}` (it only counts
/// path segments), so any dispatch path MUST try this first — otherwise a set
/// link is handed to the single-track pipeline and yt-dlp downloads the whole
/// playlist into `track.mp3`.
pub fn extract_soundcloud_set_url(text: &str) -> Option<String> {
    let url = extract_soundcloud_url(text)?;
    if is_soundcloud_set_url(&url) {
        Some(url)
    } else {
        None
    }
}

pub fn is_soundcloud_set_url(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .map(|u| {
            u.path()
                .split('/')
                .filter(|s| !s.is_empty())
                .any(|s| s.eq_ignore_ascii_case("sets"))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_set_url() {
        let text = "https://soundcloud.com/chris-467177669/sets/songs";
        assert_eq!(
            extract_soundcloud_set_url(text),
            Some("https://soundcloud.com/chris-467177669/sets/songs".to_string())
        );
    }

    #[test]
    fn test_single_track_is_not_a_set() {
        assert_eq!(
            extract_soundcloud_set_url("https://soundcloud.com/forss/vlick"),
            None
        );
    }

    #[test]
    fn test_extract_standard_soundcloud_url() {
        let text = "Check out https://soundcloud.com/forss/vlick on soundcloud";
        assert_eq!(
            extract_soundcloud_url(text),
            Some("https://soundcloud.com/forss/vlick".to_string())
        );
    }

    #[test]
    fn test_extract_short_soundcloud_url() {
        let text = "https://on.soundcloud.com/xyz123";
        assert_eq!(
            extract_soundcloud_url(text),
            Some("https://on.soundcloud.com/xyz123".to_string())
        );
    }

    #[test]
    fn test_extract_no_scheme() {
        let text = "soundcloud.com/artist/track";
        assert_eq!(
            extract_soundcloud_url(text),
            Some("https://soundcloud.com/artist/track".to_string())
        );
    }

    #[test]
    fn test_ignore_reserved_soundcloud_pages() {
        let text = "https://soundcloud.com/discover";
        assert_eq!(extract_soundcloud_url(text), None);
    }
}
