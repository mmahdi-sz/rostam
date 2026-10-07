//! YouTube search & matching candidate selection for Spotify tracks.

use anyhow::anyhow;
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const SEARCH_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_DURATION_DIFF_SECS: u64 = 8;
const MIN_SIMILARITY_SCORE: f64 = 0.45;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct YtCandidate {
    pub webpage_url: String,
    pub title: String,
    pub uploader: String,
    pub duration_secs: u64,
    pub score: f64,
}

#[allow(dead_code)]
pub async fn find_best_youtube_match(
    primary_artist: &str,
    title: &str,
    spotify_duration_ms: u64,
    trace_id: u64,
) -> anyhow::Result<YtCandidate> {
    find_best_youtube_match_fallback(primary_artist, "", "", title, spotify_duration_ms, trace_id)
        .await
}

pub async fn find_best_youtube_match_fallback(
    primary_artist: &str,
    artists_joined: &str,
    album_name: &str,
    title: &str,
    spotify_duration_ms: u64,
    trace_id: u64,
) -> anyhow::Result<YtCandidate> {
    // Build candidate queries in priority order
    let mut queries: Vec<String> = Vec::new();
    let mut seen_queries = std::collections::HashSet::new();

    let mut add_query = |q: String| {
        let trimmed = q.trim().to_string();
        if !trimmed.is_empty() && seen_queries.insert(trimmed.to_lowercase()) {
            queries.push(trimmed);
        }
    };

    // 1. Primary query: "Artist - Title"
    add_query(format!("{primary_artist} - {title}"));

    // 2. If artists_joined is present and has multiple artists, try subsequent artists
    // e.g. "Minecraft, Peter Hont" -> "Peter Hont - Title", and full joined: "Minecraft, Peter Hont - Title"
    if !artists_joined.is_empty() {
        for part in artists_joined.split([',', '&', '/']) {
            let part = part.trim();
            if !part.is_empty() && !part.eq_ignore_ascii_case(primary_artist) {
                add_query(format!("{part} - {title}"));
            }
        }
        add_query(format!("{artists_joined} - {title}"));
    }

    // 3. Try with album name if available
    if !album_name.is_empty() && !album_name.eq_ignore_ascii_case(title) {
        add_query(format!("{primary_artist} {album_name} {title}"));
        if !artists_joined.is_empty() && !artists_joined.eq_ignore_ascii_case(primary_artist) {
            add_query(format!("{artists_joined} {album_name} {title}"));
        }
    }

    // 4. Try title with "Audio" suffix
    add_query(format!("{primary_artist} - {title} Audio"));

    let mut last_err = anyhow!("No suitable YouTube match found for this track");
    for (q_idx, query) in queries.iter().enumerate() {
        if q_idx > 0 {
            log_ev!(
                "sp",
                trace_id,
                "yt_search_fallback_attempt",
                "idx" => q_idx,
                "query" => query
            );
        }
        match search_youtube_query(
            query,
            primary_artist,
            artists_joined,
            title,
            spotify_duration_ms,
            trace_id,
        )
        .await
        {
            Ok(cand) => return Ok(cand),
            Err(e) => {
                last_err = e;
            }
        }
    }

    Err(last_err)
}

async fn search_youtube_query(
    query: &str,
    primary_artist: &str,
    artists_joined: &str,
    title: &str,
    spotify_duration_ms: u64,
    trace_id: u64,
) -> anyhow::Result<YtCandidate> {
    let search_arg = format!("ytsearch5:{query}");

    log_ev!(
        "sp",
        trace_id,
        "yt_search_start",
        "query" => query,
        "spotify_dur_ms" => spotify_duration_ms
    );

    let child = Command::new("yt-dlp")
        .arg("--js-runtimes")
        .arg(format!("deno:{}", crate::config::deno_path()))
        .arg("--dump-json")
        .arg("--flat-playlist")
        .arg("--no-warnings")
        .arg("--ignore-no-formats-error")
        .arg(&search_arg)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn yt-dlp search: {e}"))?;

    let output = match tokio::time::timeout(SEARCH_TIMEOUT, child.wait_with_output()).await {
        Ok(res) => res.map_err(|e| anyhow!("Failed running yt-dlp search: {e}"))?,
        Err(_) => {
            log_ev!("sp", trace_id, "yt_search_timeout", "=>" => "timeout");
            return Err(anyhow!("YouTube search timed out after 45s"));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log_ev!(
            "sp",
            trace_id,
            "yt_search_failed",
            "err" => stderr.lines().last().unwrap_or("")
        );
        return Err(anyhow!(
            "yt-dlp search exited with error: {}",
            stderr.lines().last().unwrap_or("")
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let target_duration_secs = (spotify_duration_ms + 500) / 1000;
    let target_label_primary = format!("{primary_artist} {title}").to_lowercase();
    let target_label_joined = if !artists_joined.is_empty() {
        format!("{artists_joined} {title}").to_lowercase()
    } else {
        target_label_primary.clone()
    };

    let mut candidates: Vec<YtCandidate> = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(json) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        let webpage_url = json
            .get("webpage_url")
            .or_else(|| json.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let id = json.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let final_url = if !webpage_url.is_empty() {
            webpage_url
        } else if !id.is_empty() {
            format!("https://www.youtube.com/watch?v={id}")
        } else {
            continue;
        };

        let cand_title = json
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let uploader = json
            .get("uploader")
            .or_else(|| json.get("channel"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let duration_secs = json
            .get("duration")
            .and_then(|v| v.as_f64())
            .map(|d| d as u64)
            .unwrap_or(0);

        let dur_diff = target_duration_secs.abs_diff(duration_secs);
        if dur_diff > MAX_DURATION_DIFF_SECS {
            log_ev!(
                "sp",
                trace_id,
                "candidate_rejected_duration",
                "title" => &cand_title,
                "dur" => duration_secs,
                "diff" => dur_diff
            );
            continue;
        }

        let cand_label = format!("{cand_title} {uploader}").to_lowercase();
        let score_primary = strsim::jaro_winkler(&target_label_primary, &cand_label);
        let score_joined = strsim::jaro_winkler(&target_label_joined, &cand_label);
        let score = score_primary.max(score_joined);

        if score < MIN_SIMILARITY_SCORE {
            log_ev!(
                "sp",
                trace_id,
                "candidate_rejected_score",
                "title" => &cand_title,
                "score" => score
            );
            continue;
        }

        candidates.push(YtCandidate {
            webpage_url: final_url,
            title: cand_title,
            uploader,
            duration_secs,
            score,
        });
    }

    if candidates.is_empty() {
        log_ev!("sp", trace_id, "yt_search_no_match", "query" => query, "=>" => "no_candidates");
        return Err(anyhow!(
            "No suitable YouTube match found for query '{query}'"
        ));
    }

    // Sort by highest similarity score, breaking ties with lowest duration difference
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let diff_a = target_duration_secs.abs_diff(a.duration_secs);
                let diff_b = target_duration_secs.abs_diff(b.duration_secs);
                diff_a.cmp(&diff_b)
            })
    });

    let best = candidates[0].clone();
    log_ev!(
        "sp",
        trace_id,
        "yt_match_selected",
        "url" => &best.webpage_url,
        "title" => &best.title,
        "score" => best.score
    );

    Ok(best)
}
