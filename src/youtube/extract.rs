pub fn extract_youtube_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();

    for token in text.split(|c: char| c.is_whitespace()) {
        let token = token.trim_matches(|c: char| {
            matches!(c, '<' | '>' | '"' | '\'' | ',' | ';' | '!' | '?' | ')' | '(')
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

        if is_yt && !urls.contains(&normalized) {
            urls.push(normalized);
        }
    }

    urls
}
