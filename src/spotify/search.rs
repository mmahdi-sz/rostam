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

pub async fn find_best_youtube_match(
    primary_artist: &str,
    title: &str,
    spotify_duration_ms: u64,
    trace_id: u64,
) -> anyhow::Result<YtCandidate> {
    let query = format!("{primary_artist} - {title}");
    let search_arg = format!("ytsearch5:{query}");

    log_ev!(
        "sp",
        trace_id,
        "yt_search_start",
        "query" => &query,
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
    let target_label = format!("{primary_artist} {title}").to_lowercase();

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
        let score = strsim::jaro_winkler(&target_label, &cand_label);

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
        log_ev!("sp", trace_id, "yt_search_no_match", "=>" => "no_candidates");
        return Err(anyhow!("No suitable YouTube match found for this track"));
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
