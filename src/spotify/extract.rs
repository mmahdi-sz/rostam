//! Extract Spotify track IDs from plain text messages.

/// Extract a 22-character Spotify track ID from URLs or URIs in `text`.
///
/// Supported formats:
/// - `https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT`
/// - `https://open.spotify.com/intl-fa/track/4cOdK2wGLETKBW3PvgPWqT?si=...`
/// - `open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT`
/// - `spotify:track:4cOdK2wGLETKBW3PvgPWqT`
pub fn extract_spotify_track_id(text: &str) -> Option<String> {
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

        // Handle URI: spotify:track:{id}
        if let Some(rest) = token.strip_prefix("spotify:track:") {
            let id = rest.split(['?', '&', '#', '/']).next().unwrap_or("");
            if is_valid_spotify_id(id) {
                return Some(id.to_string());
            }
        }

        let lower = token.to_ascii_lowercase();
        let has_spotify = lower.contains("open.spotify.com");
        if !has_spotify {
            continue;
        }

        // Normalize URL if missing scheme
        let url_str = if lower.starts_with("http://") || lower.starts_with("https://") {
            token.to_string()
        } else {
            format!("https://{token}")
        };

        if let Ok(url) = reqwest::Url::parse(&url_str) {
            let path = url.path(); // e.g. "/track/4cOdK2wGLETKBW3PvgPWqT" or "/intl-fa/track/4cOdK2wGLETKBW3PvgPWqT"
            let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

            // Match track/{id}
            if let Some(pos) = segments.iter().position(|&s| s == "track") {
                if pos + 1 < segments.len() {
                    let candidate = segments[pos + 1];
                    let id = candidate.split(['?', '&', '#']).next().unwrap_or("");
                    if is_valid_spotify_id(id) {
                        return Some(id.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Album vs playlist — the embed/API path differs only in this one segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotifySetKind {
    Album,
    Playlist,
}

impl SpotifySetKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Playlist => "playlist",
        }
    }
}

/// Extract a Spotify album or playlist ID from URLs or URIs in `text`.
///
/// Same token/segment walk as `extract_spotify_track_id`; `track` is deliberately
/// not matched here so callers can branch on set-vs-track.
pub fn extract_spotify_set(text: &str) -> Option<(SpotifySetKind, String)> {
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

        for (prefix, kind) in [
            ("spotify:album:", SpotifySetKind::Album),
            ("spotify:playlist:", SpotifySetKind::Playlist),
        ] {
            if let Some(rest) = token.strip_prefix(prefix) {
                let id = rest.split(['?', '&', '#', '/']).next().unwrap_or("");
                if is_valid_spotify_id(id) {
                    return Some((kind, id.to_string()));
                }
            }
        }

        let lower = token.to_ascii_lowercase();
        if !lower.contains("open.spotify.com") {
            continue;
        }
        let url_str = if lower.starts_with("http://") || lower.starts_with("https://") {
            token.to_string()
        } else {
            format!("https://{token}")
        };

        let Ok(url) = reqwest::Url::parse(&url_str) else {
            continue;
        };
        let segments: Vec<&str> = url.path().split('/').filter(|s| !s.is_empty()).collect();
        for (seg, kind) in [
            ("album", SpotifySetKind::Album),
            ("playlist", SpotifySetKind::Playlist),
        ] {
            if let Some(pos) = segments.iter().position(|&s| s == seg) {
                if pos + 1 < segments.len() {
                    let id = segments[pos + 1]
                        .split(['?', '&', '#'])
                        .next()
                        .unwrap_or("");
                    if is_valid_spotify_id(id) {
                        return Some((kind, id.to_string()));
                    }
                }
            }
        }
    }

    None
}

/// Spotify IDs are base62 strings, typically 22 characters long.
fn is_valid_spotify_id(id: &str) -> bool {
    (20..=30).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_standard_url() {
        let msg = "Check this out https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT?si=123456";
        assert_eq!(
            extract_spotify_track_id(msg),
            Some("4cOdK2wGLETKBW3PvgPWqT".to_string())
        );
    }

    #[test]
    fn test_extract_locale_prefixed_url() {
        let msg = "https://open.spotify.com/intl-fa/track/4cOdK2wGLETKBW3PvgPWqT";
        assert_eq!(
            extract_spotify_track_id(msg),
            Some("4cOdK2wGLETKBW3PvgPWqT".to_string())
        );
    }

    #[test]
    fn test_extract_spotify_uri() {
        let msg = "spotify:track:4cOdK2wGLETKBW3PvgPWqT";
        assert_eq!(
            extract_spotify_track_id(msg),
            Some("4cOdK2wGLETKBW3PvgPWqT".to_string())
        );
    }

    #[test]
    fn test_extract_no_scheme() {
        let msg = "open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT";
        assert_eq!(
            extract_spotify_track_id(msg),
            Some("4cOdK2wGLETKBW3PvgPWqT".to_string())
        );
    }

    #[test]
    fn test_extract_album_and_playlist() {
        assert_eq!(
            extract_spotify_set("https://open.spotify.com/album/1ATL5GLyefJaxhQzSPVrLX?si=x"),
            Some((SpotifySetKind::Album, "1ATL5GLyefJaxhQzSPVrLX".to_string()))
        );
        assert_eq!(
            extract_spotify_set("https://open.spotify.com/intl-fa/playlist/37i9dQZF1DXcBWIGoYBM5M"),
            Some((
                SpotifySetKind::Playlist,
                "37i9dQZF1DXcBWIGoYBM5M".to_string()
            ))
        );
        assert_eq!(
            extract_spotify_set("spotify:album:1ATL5GLyefJaxhQzSPVrLX"),
            Some((SpotifySetKind::Album, "1ATL5GLyefJaxhQzSPVrLX".to_string()))
        );
    }

    #[test]
    fn test_track_url_is_not_a_set() {
        assert_eq!(
            extract_spotify_set("https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT"),
            None
        );
    }

    #[test]
    fn test_non_spotify_url() {
        let msg = "https://youtube.com/watch?v=dQw4w9WgXcQ";
        assert_eq!(extract_spotify_track_id(msg), None);
    }
}
