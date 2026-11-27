use std::process::Stdio;
use std::time::Duration;

use crate::surge_dl::types::SurgeDetail;

pub(crate) fn surge_cmd(args: &[&str]) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("surge");
    cmd.args(args)
        .arg("--host")
        .arg(crate::config::surge_host());
    if let Some(token) = crate::config::surge_token() {
        cmd.arg("--token").arg(token);
    }
    cmd
}

#[derive(Debug)]
pub(crate) enum SurgeAddError {
    Spawn(std::io::ErrorKind),
    Wait(std::io::ErrorKind),
    Timeout,
    Connection,
    Rejected(u16),
    Exit(Option<i32>),
}

impl std::fmt::Display for SurgeAddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(kind) => write!(f, "spawn_failed kind={kind:?}"),
            Self::Wait(kind) => write!(f, "wait_failed kind={kind:?}"),
            Self::Timeout => f.write_str("timeout"),
            Self::Connection => f.write_str("connection_failed"),
            Self::Rejected(status) => write!(f, "request_rejected status={status}"),
            Self::Exit(code) => write!(f, "cli_failed exit={code:?}"),
        }
    }
}

fn classify_add_error(stderr: &[u8], exit: Option<i32>) -> SurgeAddError {
    let stderr = String::from_utf8_lossy(stderr);
    for line in stderr.lines() {
        let detail = line
            .strip_prefix("Error adding ")
            .and_then(|line| line.split_once(": "))
            .map_or(line, |(_, detail)| detail);
        if detail.starts_with("failed to connect to server:") {
            return SurgeAddError::Connection;
        }
        if let Some(status) = detail
            .strip_prefix("server error: ")
            .or_else(|| detail.strip_prefix("server returned status "))
            .or_else(|| detail.strip_prefix("unexpected status code: "))
            .and_then(|status| status.split_whitespace().next()?.parse::<u16>().ok())
            .filter(|status| (400..600).contains(status))
        {
            return SurgeAddError::Rejected(status);
        }
    }
    SurgeAddError::Exit(exit)
}

pub(crate) async fn run_surge_add(url: &str, dir: &str) -> Result<(), SurgeAddError> {
    let child = surge_cmd(&["add", url, "-o", dir])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| SurgeAddError::Spawn(err.kind()))?;
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| SurgeAddError::Timeout)?
        .map_err(|err| SurgeAddError::Wait(err.kind()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(classify_add_error(&output.stderr, output.status.code()))
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

#[cfg(test)]
mod add_tests {
    use super::*;

    #[test]
    fn redacts_cli_output_and_keeps_failure_reason() {
        let connection = b"Error adding https://example.test/?token=secret: failed to connect to server: connection refused";
        let rejected = b"Error adding https://example.test/?token=secret: server error: 401 Unauthorized - body";
        assert_eq!(
            classify_add_error(connection, Some(1)).to_string(),
            "connection_failed"
        );
        assert_eq!(
            classify_add_error(rejected, Some(1)).to_string(),
            "request_rejected status=401"
        );
        assert_eq!(
            classify_add_error(b"secret", Some(2)).to_string(),
            "cli_failed exit=Some(2)"
        );
    }
}
