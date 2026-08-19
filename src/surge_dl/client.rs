use std::process::Stdio;
use std::time::Duration;

use crate::surge_dl::types::SurgeDetail;

pub(crate) fn surge_cmd(args: &[&str]) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("surge");
    cmd.args(args)
        .arg("--host")
        .arg(crate::config::surge_host());
    cmd
}

pub(crate) async fn run_surge_add(url: &str, dir: &str) -> bool {
    let mut cmd = surge_cmd(&["add", url, "-o", dir]);
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
        Ok(Ok(status)) => status.success(),
        _ => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            false
        }
    }
}

pub(crate) async fn list_surge_job_ids() -> Vec<String> {
    let output = surge_cmd(&["ls", "--json"]).output().await;
    let Ok(output) = output else {
        return vec![];
    };
    let entries: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    entries
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("id")?.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) async fn find_job_id_by_url(
    url: &str,
    before_ids: &[String],
    trace_id: u64,
) -> Option<String> {
    for _ in 0..10 {
        let output = surge_cmd(&["ls", "--json"]).output().await;
        let Ok(output) = output else {
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        };
        let entries: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
        if let Some(arr) = entries.as_array() {
            let mut candidate_ids: Vec<String> = arr
                .iter()
                .rev()
                .filter_map(|e| e.get("id")?.as_str().map(str::to_string))
                .filter(|id| !before_ids.contains(id))
                .collect();

            if candidate_ids.is_empty() {
                candidate_ids = arr
                    .iter()
                    .rev()
                    .take(10)
                    .filter_map(|e| e.get("id")?.as_str().map(str::to_string))
                    .collect();
            }

            for id in candidate_ids {
                if let Some(detail) = fetch_detail(&id).await {
                    if detail.url == url {
                        return Some(id);
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    log_ev!("surge_dl", trace_id, "find_job_id_timeout", "url" => url);
    None
}

pub(crate) async fn fetch_detail(id: &str) -> Option<SurgeDetail> {
    let output = surge_cmd(&["ls", id, "--json"]).output().await.ok()?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    Some(SurgeDetail {
        filename: json.get("filename")?.as_str()?.to_string(),
        url: json.get("url")?.as_str()?.to_string(),
        total_size: json.get("total_size")?.as_u64()?,
        downloaded: json.get("downloaded")?.as_u64()?,
        progress: json.get("progress")?.as_f64()?,
        speed: json.get("speed").and_then(|v| v.as_f64()).unwrap_or(0.0),
        avg_speed: json
            .get("avg_speed")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        status: json.get("status")?.as_str()?.to_string(),
    })
}
