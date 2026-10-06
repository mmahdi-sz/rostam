//! Spotify Web API client wrapper using `rspotify` with public embed fallback.

use anyhow::{Context, anyhow};
use rspotify::{ClientCredsSpotify, Credentials, model::TrackId, prelude::*};
use std::time::Duration;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SpotifyTrackMeta {
    pub id: String,
    pub title: String,
    pub primary_artist: String,
    pub artists_joined: String,
    pub album_name: String,
    pub duration_ms: u64,
    pub cover_url: Option<String>,
    pub release_date: Option<String>,
}

pub async fn fetch_spotify_track(track_id: &str) -> anyhow::Result<SpotifyTrackMeta> {
    match fetch_spotify_track_api(track_id).await {
        Ok(meta) => Ok(meta),
        Err(e) => {
            // Only log when credentials ARE configured but still failed (real error).
            // Missing credentials is by-design; don't spam.
            if crate::config::spotify_client_id().is_some() {
                log_ev!("sp", 0, "api_fetch_failed_trying_public_fallback", "err" => e.to_string());
            }
            fetch_spotify_track_public(track_id).await
        }
    }
}

async fn fetch_spotify_track_api(track_id: &str) -> anyhow::Result<SpotifyTrackMeta> {
    let client_id = crate::config::spotify_client_id().ok_or_else(|| {
        anyhow!("SPOTIFY_CLIENT_ID is not configured in environment or config file")
    })?;
    let client_secret = crate::config::spotify_client_secret().ok_or_else(|| {
        anyhow!("SPOTIFY_CLIENT_SECRET is not configured in environment or config file")
    })?;

    let creds = Credentials::new(&client_id, &client_secret);
    let spotify = ClientCredsSpotify::new(creds);

    spotify
        .request_token()
        .await
        .context("Failed to request Spotify API client credentials token")?;

    let tid = TrackId::from_id(track_id)
        .map_err(|e| anyhow!("Invalid Spotify track ID format '{track_id}': {e}"))?;

    let track = spotify
        .track(tid, None)
        .await
        .map_err(|e| anyhow!("Spotify API error for track '{track_id}': {e}"))?;

    let primary_artist = track
        .artists
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "Unknown Artist".to_string());

    let artist_names: Vec<String> = track.artists.iter().map(|a| a.name.clone()).collect();
    let artists_joined = if artist_names.is_empty() {
        "Unknown Artist".to_string()
    } else {
        artist_names.join(", ")
    };

    let album_name = track.album.name.clone();
    let release_date = track.album.release_date.clone();

    let cover_url = track
        .album
        .images
        .iter()
        .max_by_key(|img| img.width.unwrap_or(0) * img.height.unwrap_or(0))
        .map(|img| img.url.clone());

    let duration_ms = track.duration.num_milliseconds() as u64;

    Ok(SpotifyTrackMeta {
        id: track_id.to_string(),
        title: track.name,
        primary_artist,
        artists_joined,
        album_name,
        duration_ms,
        cover_url,
        release_date,
    })
}

/// One entry of an album/playlist — enough to feed the single-track pipeline.
#[derive(Debug, Clone)]
pub struct SpotifySetItem {
    pub track_id: String,
    pub title: String,
    pub artist: String,
}

#[derive(Debug, Clone)]
pub struct SpotifySet {
    pub title: String,
    pub items: Vec<SpotifySetItem>,
}

/// Maximum tracks allowed from a single Spotify playlist or album as defense-in-depth against runaway lists.
pub const MAX_SPOTIFY_PLAYLIST_TRACKS: usize = 2000;
#[allow(dead_code)]
pub const MAX_SPOTIFY_SET_TRACKS: usize = MAX_SPOTIFY_PLAYLIST_TRACKS;

static SPOTIFY_EMBED_TOKEN_CACHE: tokio::sync::RwLock<Option<(String, std::time::Instant)>> =
    tokio::sync::RwLock::const_new(None);

/// Get an anonymous embed session token from cache or fetch a fresh one from a lightweight embed page.
pub async fn get_cached_or_fresh_embed_token(client: &reqwest::Client) -> Option<String> {
    {
        let read = SPOTIFY_EMBED_TOKEN_CACHE.read().await;
        if let Some((ref token, created)) = *read {
            if created.elapsed() < Duration::from_secs(3600) {
                return Some(token.clone());
            }
        }
    }

    let url = "https://open.spotify.com/embed/track/4cOdK2wGLETKBW3PvgPWqT";
    if let Ok(resp) = client.get(url).send().await {
        if let Ok(html) = resp.text().await {
            if let Some(next_data) = embed_next_data(&html) {
                if let Some(token) = extract_embed_token(&next_data) {
                    let mut write = SPOTIFY_EMBED_TOKEN_CACHE.write().await;
                    *write = Some((token.clone(), std::time::Instant::now()));
                    return Some(token);
                }
            }
        }
    }
    None
}

/// Extract the anonymous session accessToken embedded in Spotify's __NEXT_DATA__ JSON.
pub fn extract_embed_token(data: &serde_json::Value) -> Option<String> {
    const TOKEN_PATHS: &[&str] = &[
        "/props/pageProps/state/settings/session/accessToken",
        "/props/pageProps/settings/session/accessToken",
        "/props/pageProps/session/accessToken",
        "/props/pageProps/state/data/session/accessToken",
    ];
    for path in TOKEN_PATHS {
        if let Some(token) = data.pointer(path).and_then(|v| v.as_str()) {
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// Parse full track IDs from Spotify's internal spclient endpoint response.
pub fn parse_spclient_track_ids(json_str: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return Vec::new();
    };
    let items = v
        .pointer("/contents/items")
        .and_then(|arr| arr.as_array())
        .map(|a| a.as_slice())
        .unwrap_or_default();

    let mut track_ids = Vec::with_capacity(items.len());
    for item in items {
        if let Some(uri) = item.get("uri").and_then(|u| u.as_str()) {
            if let Some(track_id) = uri.strip_prefix("spotify:track:") {
                if !track_id.is_empty() {
                    track_ids.push(track_id.to_string());
                    if track_ids.len() >= MAX_SPOTIFY_PLAYLIST_TRACKS {
                        break;
                    }
                }
            }
        }
    }
    track_ids
}

/// Fetch an album/playlist/artist track list from the public embed page, extending
/// albums and playlists beyond 100 tracks via Spotify's internal APIs when available.
pub async fn fetch_spotify_set(
    kind: crate::spotify::extract::SpotifySetKind,
    set_id: &str,
) -> anyhow::Result<SpotifySet> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(Duration::from_secs(20))
        .build()?;

    if kind == crate::spotify::extract::SpotifySetKind::Artist {
        return fetch_spotify_artist_top_tracks(&client, set_id).await;
    }

    let embed_url = format!("https://open.spotify.com/embed/{}/{set_id}", kind.as_str());
    let html = client.get(&embed_url).send().await?.text().await?;
    // Private/deleted playlist embed returns 404; ?pt= link won't open it either, so handle separately for proper error message.
    if embed_status(&html) == Some(404) {
        return Err(anyhow!(
            "spotify embed 404: {} '{set_id}' is private or deleted",
            kind.as_str()
        ));
    }
    let next_data = embed_next_data(&html);

    if let Some(ref tok) = next_data.as_ref().and_then(extract_embed_token) {
        let mut write = SPOTIFY_EMBED_TOKEN_CACHE.write().await;
        *write = Some((tok.clone(), std::time::Instant::now()));
    }

    let entity = extract_embed_entity(&html).ok_or_else(|| {
        anyhow!(
            "Failed to parse embed page for {} '{set_id}'",
            kind.as_str()
        )
    })?;

    let title = entity
        .get("name")
        .or_else(|| entity.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let mut items = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for entry in entity
        .get("trackList")
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or_default()
    {
        // uri looks like `spotify:track:<id>`; anything else (episode, local
        // file) has no downloadable track id and is skipped.
        let Some(track_id) = entry
            .get("uri")
            .and_then(|v| v.as_str())
            .and_then(|u| u.strip_prefix("spotify:track:"))
        else {
            continue;
        };
        seen_ids.insert(track_id.to_string());
        items.push(SpotifySetItem {
            track_id: track_id.to_string(),
            title: entry
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            artist: entry
                .get("subtitle")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown Artist")
                .to_string(),
        });
    }

    // For albums, if an anonymous session token is present, query Spotify's partner GraphQL pathfinder API
    // to fetch all tracks (up to 2000) with canonical titles, artist credits, and IDs.
    if kind == crate::spotify::extract::SpotifySetKind::Album {
        let token = match next_data.as_ref().and_then(extract_embed_token) {
            Some(t) => t,
            None => get_cached_or_fresh_embed_token(&client)
                .await
                .unwrap_or_default(),
        };

        if !token.is_empty() {
            if let Ok(mut album_url) =
                reqwest::Url::parse("https://api-partner.spotify.com/pathfinder/v1/query")
            {
                album_url.query_pairs_mut()
                    .append_pair("operationName", "getAlbum")
                    .append_pair(
                        "variables",
                        &serde_json::json!({
                            "uri": format!("spotify:album:{set_id}"),
                            "locale": "",
                            "offset": 0,
                            "limit": MAX_SPOTIFY_PLAYLIST_TRACKS
                        })
                        .to_string(),
                    )
                    .append_pair(
                        "extensions",
                        &serde_json::json!({
                            "persistedQuery": {
                                "version": 1,
                                "sha256Hash": "b9bfabef66ed756e5e13f68a942deb60bd4125ec1f1be8cc42769dc0259b4b10"
                            }
                        })
                        .to_string(),
                    );

                let album_req = client
                    .get(album_url)
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Accept", "application/json")
                    .timeout(Duration::from_secs(15))
                    .send()
                    .await;

                match album_req {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(body) = resp.json::<serde_json::Value>().await {
                            if let Some(track_items) = body
                                .pointer("/data/albumUnion/tracksV2/items")
                                .and_then(|v| v.as_array())
                            {
                                if track_items.len() > items.len()
                                    || (!track_items.is_empty() && items.is_empty())
                                {
                                    log_ev!(
                                        "sp",
                                        0,
                                        "pathfinder_expanded_album",
                                        "embed_tracks" => items.len(),
                                        "total_tracks" => track_items.len()
                                    );
                                    let mut expanded_items = Vec::with_capacity(track_items.len());
                                    for item in track_items {
                                        if let Some(track) = item.get("track") {
                                            if let Some(uri) =
                                                track.get("uri").and_then(|u| u.as_str())
                                            {
                                                if let Some(track_id) =
                                                    uri.strip_prefix("spotify:track:")
                                                {
                                                    let t_title = track
                                                        .get("name")
                                                        .and_then(|n| n.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    let t_artists = track
                                                        .pointer("/artists/items")
                                                        .and_then(|arr| arr.as_array())
                                                        .map(|arr| {
                                                            arr.iter()
                                                                .filter_map(|a| {
                                                                    a.pointer("/profile/name")
                                                                        .and_then(|n| n.as_str())
                                                                })
                                                                .collect::<Vec<_>>()
                                                                .join(", ")
                                                        })
                                                        .unwrap_or_default();
                                                    expanded_items.push(SpotifySetItem {
                                                        track_id: track_id.to_string(),
                                                        title: t_title,
                                                        artist: t_artists,
                                                    });
                                                    if expanded_items.len()
                                                        >= MAX_SPOTIFY_PLAYLIST_TRACKS
                                                    {
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if !expanded_items.is_empty() {
                                        items = expanded_items;
                                    }
                                }
                            }
                        }
                    }
                    Ok(resp) => {
                        log_ev!(
                            "sp",
                            0,
                            "pathfinder_fetch_status_err",
                            "status" => resp.status().as_u16()
                        );
                    }
                    Err(e) => {
                        log_ev!("sp", 0, "pathfinder_fetch_err", "err" => e.to_string());
                    }
                }
            }
        }
    }

    // For playlists, if an anonymous session token is present in the embed page,
    // query Spotify's internal spclient endpoint to extend tracks beyond the 100-track embed ceiling.
    if kind == crate::spotify::extract::SpotifySetKind::Playlist {
        if let Some(token) = next_data.as_ref().and_then(extract_embed_token) {
            let spclient_url =
                format!("https://spclient.wg.spotify.com/playlist/v2/playlist/{set_id}");
            let spclient_req = client
                .get(&spclient_url)
                .header("Authorization", format!("Bearer {token}"))
                .header("Accept", "application/json")
                .timeout(Duration::from_secs(15))
                .send()
                .await;

            match spclient_req {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(body) = resp.text().await {
                        let all_track_ids = parse_spclient_track_ids(&body);
                        if all_track_ids.len() > items.len() {
                            log_ev!(
                                "sp",
                                0,
                                "spclient_expanded_playlist",
                                "embed_tracks" => items.len(),
                                "total_tracks" => all_track_ids.len()
                            );
                            for tid in all_track_ids {
                                if seen_ids.insert(tid.clone()) {
                                    let track_num = items.len() + 1;
                                    items.push(SpotifySetItem {
                                        track_id: tid,
                                        title: format!("Track {track_num}"),
                                        artist: String::new(),
                                    });
                                    if items.len() >= MAX_SPOTIFY_PLAYLIST_TRACKS {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(resp) => {
                    log_ev!(
                        "sp",
                        0,
                        "spclient_fetch_status_err",
                        "status" => resp.status().as_u16()
                    );
                }
                Err(e) => {
                    log_ev!("sp", 0, "spclient_fetch_err", "err" => e.to_string());
                }
            }
        }
    }

    if items.is_empty() {
        return Err(anyhow!(
            "No playable tracks in {} '{set_id}'",
            kind.as_str()
        ));
    }

    Ok(SpotifySet { title, items })
}

/// Fetch an artist's top tracks via Spotify partner pathfinder GraphQL API.
async fn fetch_spotify_artist_top_tracks(
    client: &reqwest::Client,
    artist_id: &str,
) -> anyhow::Result<SpotifySet> {
    let token = get_cached_or_fresh_embed_token(client)
        .await
        .ok_or_else(|| {
            anyhow!("Failed to obtain anonymous token for Spotify artist '{artist_id}'")
        })?;

    let mut url = reqwest::Url::parse("https://api-partner.spotify.com/pathfinder/v1/query")?;
    url.query_pairs_mut()
        .append_pair("operationName", "queryArtistOverview")
        .append_pair(
            "variables",
            &serde_json::json!({
                "uri": format!("spotify:artist:{artist_id}"),
                "locale": ""
            })
            .to_string(),
        )
        .append_pair(
            "extensions",
            &serde_json::json!({
                "persistedQuery": {
                    "version": 1,
                    "sha256Hash": "7f86ff63e38c24973a2842b672abe44c910c1973978dc8a4a0cb648edef34527"
                }
            })
            .to_string(),
        );

    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(15))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow!(
            "Failed to fetch artist overview for '{artist_id}': status {}",
            resp.status()
        ));
    }

    let body: serde_json::Value = resp.json().await?;
    let artist_union = body
        .pointer("/data/artistUnion")
        .ok_or_else(|| anyhow!("Artist '{artist_id}' not found"))?;

    let artist_name = artist_union
        .pointer("/profile/name")
        .and_then(|v| v.as_str())
        .unwrap_or("Artist")
        .to_string();

    let title = format!("{artist_name} - Top Tracks");
    let top_items = artist_union
        .pointer("/discography/topTracks/items")
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or_default();

    let mut items = Vec::new();
    for item in top_items {
        if let Some(track) = item.get("track") {
            if let Some(uri) = track.get("uri").and_then(|u| u.as_str()) {
                if let Some(track_id) = uri.strip_prefix("spotify:track:") {
                    let track_title = track
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let artist_names = track
                        .pointer("/artists/items")
                        .and_then(|arr| arr.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|a| a.pointer("/profile/name").and_then(|n| n.as_str()))
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_else(|| artist_name.clone());

                    items.push(SpotifySetItem {
                        track_id: track_id.to_string(),
                        title: track_title,
                        artist: artist_names,
                    });
                }
            }
        }
    }

    if items.is_empty() {
        return Err(anyhow!("No top tracks found for artist '{artist_id}'"));
    }

    Ok(SpotifySet { title, items })
}

fn embed_next_data(html: &str) -> Option<serde_json::Value> {
    const MARKER: &str = "id=\"__NEXT_DATA__\" type=\"application/json\">";
    let start = html.find(MARKER)? + MARKER.len();
    let end = html[start..].find("</script>")?;
    serde_json::from_str(&html[start..start + end]).ok()
}

fn embed_status(html: &str) -> Option<u64> {
    embed_next_data(html)?
        .pointer("/props/pageProps/status")?
        .as_u64()
}

fn extract_embed_entity(html: &str) -> Option<serde_json::Value> {
    embed_next_data(html)?
        .pointer("/props/pageProps/state/data/entity")
        .cloned()
}

/// Fallback method to parse track metadata directly from Spotify's public embed pages.
/// Works without requiring Spotify Developer Credentials or a Premium subscription.
async fn fetch_spotify_track_public(track_id: &str) -> anyhow::Result<SpotifyTrackMeta> {
    let client = crate::http::client();

    let embed_url = format!("https://open.spotify.com/embed/track/{track_id}");
    if let Ok(resp) = client.get(&embed_url).send().await {
        if let Ok(html) = resp.text().await {
            if let Some(start_idx) = html.find("id=\"__NEXT_DATA__\" type=\"application/json\">") {
                let json_start =
                    start_idx + "id=\"__NEXT_DATA__\" type=\"application/json\">".len();
                if let Some(end_idx) = html[json_start..].find("</script>") {
                    let json_str = &html[json_start..json_start + end_idx];
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
                        if let Some(entity) = v.pointer("/props/pageProps/state/data/entity") {
                            let title = entity
                                .get("title")
                                .or_else(|| entity.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            let mut artist_names = Vec::new();
                            if let Some(arr) = entity.get("artists").and_then(|v| v.as_array()) {
                                for a in arr {
                                    if let Some(name) = a.get("name").and_then(|v| v.as_str()) {
                                        artist_names.push(name.to_string());
                                    }
                                }
                            }
                            let primary_artist = artist_names
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "Unknown Artist".to_string());
                            let artists_joined = if artist_names.is_empty() {
                                "Unknown Artist".to_string()
                            } else {
                                artist_names.join(", ")
                            };

                            let duration_ms =
                                entity.get("duration").and_then(|v| v.as_u64()).unwrap_or(0);

                            let release_date = entity
                                .get("releaseDate")
                                .and_then(|v| v.get("isoString"))
                                .or_else(|| entity.get("release_date"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());

                            let mut cover_url = None;
                            if let Some(arr) = entity
                                .pointer("/visualIdentity/image")
                                .and_then(|v| v.as_array())
                            {
                                cover_url = arr
                                    .first()
                                    .and_then(|img| img.get("url"))
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                            }

                            if !title.is_empty() {
                                return Ok(SpotifyTrackMeta {
                                    id: track_id.to_string(),
                                    title: title.clone(),
                                    primary_artist,
                                    artists_joined,
                                    album_name: title,
                                    duration_ms,
                                    cover_url,
                                    release_date,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // Secondary fallback: oEmbed
    let oembed_url =
        format!("https://open.spotify.com/oembed?url=https://open.spotify.com/track/{track_id}");
    let oembed_resp: serde_json::Value = client.get(&oembed_url).send().await?.json().await?;
    let raw_title = oembed_resp
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let cover_url = oembed_resp
        .get("thumbnail_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let (title, primary_artist, artists_joined) = if let Some((t, a)) = raw_title.split_once(" - ")
    {
        (
            t.trim().to_string(),
            a.trim().to_string(),
            a.trim().to_string(),
        )
    } else {
        (
            raw_title.clone(),
            "Unknown Artist".to_string(),
            "Unknown Artist".to_string(),
        )
    };

    if !title.is_empty() {
        Ok(SpotifyTrackMeta {
            id: track_id.to_string(),
            title: title.clone(),
            primary_artist,
            artists_joined,
            album_name: title,
            duration_ms: 180000,
            cover_url,
            release_date: None,
        })
    } else {
        Err(anyhow!(
            "Failed to parse Spotify metadata for track '{track_id}'"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_embed_token_primary_path() {
        let json: serde_json::Value = serde_json::json!({
            "props": {
                "pageProps": {
                    "state": {
                        "settings": {
                            "session": {
                                "accessToken": "BQCTestToken123456"
                            }
                        }
                    }
                }
            }
        });
        assert_eq!(
            extract_embed_token(&json),
            Some("BQCTestToken123456".to_string())
        );
    }

    #[test]
    fn test_extract_embed_token_fallback_path() {
        let json: serde_json::Value = serde_json::json!({
            "props": {
                "pageProps": {
                    "session": {
                        "accessToken": "BQCFallbackToken789"
                    }
                }
            }
        });
        assert_eq!(
            extract_embed_token(&json),
            Some("BQCFallbackToken789".to_string())
        );
    }

    #[test]
    fn test_extract_embed_token_missing() {
        let json: serde_json::Value = serde_json::json!({
            "props": {
                "pageProps": {}
            }
        });
        assert_eq!(extract_embed_token(&json), None);
    }

    #[test]
    fn test_parse_spclient_track_ids() {
        let payload = r#"{
            "length": 3,
            "contents": {
                "items": [
                    {"uri": "spotify:track:1111111111111111111111"},
                    {"uri": "spotify:episode:ignore_this_podcast"},
                    {"uri": "spotify:track:2222222222222222222222"},
                    {"uri": "spotify:track:3333333333333333333333"}
                ]
            }
        }"#;
        let track_ids = parse_spclient_track_ids(payload);
        assert_eq!(
            track_ids,
            vec![
                "1111111111111111111111".to_string(),
                "2222222222222222222222".to_string(),
                "3333333333333333333333".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn test_fetch_spotify_set_real_large_playlist() {
        let set = fetch_spotify_set(
            crate::spotify::extract::SpotifySetKind::Playlist,
            "4HwaPDuT7HkO9PqudOErY5",
        )
        .await
        .unwrap();
        assert_eq!(set.title, "Breakcore bangers");
        assert!(
            set.items.len() >= 700,
            "expected >= 700 tracks, got {}",
            set.items.len()
        );
    }

    #[tokio::test]
    async fn test_fetch_spotify_set_real_large_album() {
        let set = fetch_spotify_set(
            crate::spotify::extract::SpotifySetKind::Album,
            "4hc3HIPQ1zWX6X6wrWjYa5",
        )
        .await
        .unwrap();
        assert_eq!(
            set.title,
            "Minecraft Dungeons II (Original Game Soundtrack)"
        );
        assert_eq!(
            set.items.len(),
            210,
            "expected exactly 210 tracks from pathfinder expansion, got {}",
            set.items.len()
        );
        assert_eq!(set.items[0].title, "Press Any Key");
    }

    #[tokio::test]
    async fn test_fetch_spotify_set_real_artist() {
        let set = fetch_spotify_set(
            crate::spotify::extract::SpotifySetKind::Artist,
            "0gxyHStUsqpMadRV0Di1Qt",
        )
        .await
        .unwrap();
        assert_eq!(set.title, "Rick Astley - Top Tracks");
        assert!(
            !set.items.is_empty(),
            "expected artist top tracks, got {}",
            set.items.len()
        );
    }

    #[tokio::test]
    async fn test_fetch_spotify_track_public_fallback() {
        let meta = fetch_spotify_track_public("2gHb5m0xVh988qx8K6YPQd")
            .await
            .unwrap();
        assert_eq!(meta.title, "Intizar");
        assert_eq!(meta.primary_artist, "Baylarlii");
        assert!(meta.duration_ms > 0);
        assert!(meta.cover_url.is_some());
        assert!(meta.release_date.is_some());
    }
}
