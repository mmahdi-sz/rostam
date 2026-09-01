pub fn extract_youtube_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();

    for token in text.split(|c: char| c.is_whitespace()) {
        let token = token.trim_matches(|c: char| {
            matches!(
                c,
                '<' | '>' | '"' | '\'' | ',' | ';' | '!' | '?' | ')' | '('
            )
        });

        if token.is_empty() {
            continue;
        }

        let lower = token.to_ascii_lowercase();
        let normalized = if lower.starts_with("http://") || lower.starts_with("https://") {
            token.to_string()
        } else if lower.starts_with("www.youtube.com")
            || lower.starts_with("youtube.com")
            || lower.starts_with("m.youtube.com")
            || lower.starts_with("youtu.be")
        {
            format!("https://{token}")
        } else {
            continue;
        };

        // Handle concatenated URLs by stripping trailing glued schemes.
        let lower_norm = normalized.to_ascii_lowercase();
        let second_scheme = [
            lower_norm[1..].find("http://"),
            lower_norm[1..].find("https://"),
        ]
        .into_iter()
        .flatten()
        .min();
        let normalized = match second_scheme {
            Some(i) => normalized[..i + 1].to_string(),
            None => normalized,
        };

        let host_part = normalized
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("")
            .to_ascii_lowercase();

        let is_yt = host_part == "youtu.be"
            || host_part == "youtube.com"
            || host_part == "www.youtube.com"
            || host_part == "m.youtube.com"
            || host_part == "music.youtube.com";

        if is_yt {
            let cleaned = clean_youtube_url(&normalized);
            if !urls.contains(&cleaned) {
                urls.push(cleaned);
            }
        }
    }

    urls
}

fn clean_youtube_url(url: &str) -> String {
    // Some URLs copied from Google Search have URL-encoded delimiters
    let url_decoded = url.replace("%3F", "?").replace("%3D", "=");
    let mut parts = url_decoded.splitn(2, '?');
    let base = parts.next().unwrap_or("");
    let query_str = match parts.next() {
        Some(q) => q,
        None => return url.to_string(),
    };

    let mut query_parts = query_str.splitn(2, '#');
    let query = query_parts.next().unwrap_or("");
    let fragment = query_parts.next();

    let allowed_keys = ["v", "list", "index", "t"];
    let mut kept_params = Vec::new();

    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let key = pair.split('=').next().unwrap_or("");
        if allowed_keys.contains(&key) {
            kept_params.push(pair);
        }
    }

    if kept_params.is_empty() {
        if let Some(f) = fragment {
            format!("{base}#{f}")
        } else {
            base.to_string()
        }
    } else {
        let new_query = kept_params.join("&");
        if let Some(f) = fragment {
            format!("{base}?{new_query}#{f}")
        } else {
            format!("{base}?{new_query}")
        }
    }
}

/// Returns true if the URL points to a YouTube channel or channel tab rather than a specific video or playlist.
pub fn is_youtube_channel_url(raw_url: &str) -> bool {
    let normalized = if !raw_url.contains("://") {
        format!("https://{raw_url}")
    } else {
        raw_url.to_string()
    };
    let after_scheme = match normalized.split("://").nth(1) {
        Some(rest) => rest,
        None => return false,
    };
    let path_and_query = match after_scheme.find('/') {
        Some(idx) => &after_scheme[idx..],
        None => return false,
    };
    let path = path_and_query
        .split('?')
        .next()
        .unwrap_or("")
        .split('#')
        .next()
        .unwrap_or("");
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return false;
    }
    // 1. Channel handles: /@username or /@username/<tab>
    if segments[0].starts_with('@') {
        if !raw_url.contains("watch?") && !raw_url.contains("v=") {
            return true;
        }
    }
    // 2. /channel/..., /c/..., /user/...
    if matches!(segments[0], "channel" | "c" | "user")
        && !raw_url.contains("watch?")
        && !raw_url.contains("v=")
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_plain_youtube_url() {
        let msg = "https://www.youtube.com/watch%3Fv%3DzNb2DjlZ530&ved=2ahUKEwjS9I2yuPOVAxUe8bsIHTbPDqwQwqsBegQIFBAB&sqi=2&usg=AOvVaw1ZF2BrTuDvKqmX-QRuWxOy ببین";
        let urls = extract_youtube_urls(msg);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://www.youtube.com/watch?v=zNb2DjlZ530");
    }

    #[test]
    fn test_extract_youtu_be_short_url() {
        let msg = "https://youtu.be/dQw4w9WgXcQ";
        let urls = extract_youtube_urls(msg);
        assert_eq!(urls.len(), 1);
    }

    #[test]
    fn test_no_youtube_url() {
        let msg = "این یک متن معمولی بدون لینک یوتیوب است";
        let urls = extract_youtube_urls(msg);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_two_urls_glued_without_space() {
        let msg = "https://www.youtube.com/playlist?list=PLabc123https://youtu.be/dQw4w9WgXcQ";
        let urls = extract_youtube_urls(msg);
        assert_eq!(urls[0], "https://www.youtube.com/playlist?list=PLabc123");
    }

    #[test]
    fn test_extract_without_scheme() {
        let msg = "youtube.com/watch?v=dQw4w9WgXcQ";
        let urls = extract_youtube_urls(msg);
        assert_eq!(urls.len(), 1);
        assert!(urls[0].starts_with("https://"));
    }

    #[test]
    fn test_extract_from_multiline_persian_post() {
        let msg = "احتمالا شما هم روزانه کلی اخبار مربوط به هوش مصنوعی و ایجنت ها می بینین...\nhttps://youtu.be/eicAD-UOn-c?si=cnW_EFA_n4KQRh9v\n#هوش_مصنوعی";
        let urls = extract_youtube_urls(msg);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://youtu.be/eicAD-UOn-c");
    }

    #[test]
    fn test_is_youtube_channel_url() {
        assert!(is_youtube_channel_url("https://www.youtube.com/@EasyPeasyEnglish/shorts"));
        assert!(is_youtube_channel_url("https://www.youtube.com/@EasyPeasyEnglish/streams"));
        assert!(is_youtube_channel_url("https://www.youtube.com/@EasyPeasyEnglish/podcasts"));
        assert!(is_youtube_channel_url("https://www.youtube.com/@EasyPeasyEnglish"));
        assert!(is_youtube_channel_url("https://www.youtube.com/@EasyPeasyEnglish/"));
        assert!(is_youtube_channel_url("https://www.youtube.com/channel/UC123456789"));
        assert!(is_youtube_channel_url("https://www.youtube.com/c/SomeChannel"));
        assert!(is_youtube_channel_url("https://www.youtube.com/user/SomeUser"));

        assert!(!is_youtube_channel_url("https://www.youtube.com/shorts/bQVU_L-5dDM"));
        assert!(!is_youtube_channel_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(!is_youtube_channel_url("https://youtu.be/dQw4w9WgXcQ"));
        assert!(!is_youtube_channel_url("https://www.youtube.com/playlist?list=PLsrak_Tdck7WxloYLlh6mH17IxMyk2tyl"));
    }
}
