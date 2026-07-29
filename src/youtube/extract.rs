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
            format!("https://{}", token)
        } else {
            continue;
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
            format!("{}#{}", base, f)
        } else {
            base.to_string()
        }
    } else {
        let new_query = kept_params.join("&");
        if let Some(f) = fragment {
            format!("{}?{}#{}", base, new_query, f)
        } else {
            format!("{}?{}", base, new_query)
        }
    }
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
    fn test_extract_without_scheme() {
        let msg = "youtube.com/watch?v=dQw4w9WgXcQ";
        let urls = extract_youtube_urls(msg);
        assert_eq!(urls.len(), 1);
        assert!(urls[0].starts_with("https://"));
    }
}
