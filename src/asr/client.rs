use std::path::Path;
use std::time::Duration;

#[derive(Debug)]
pub struct AsrResult {
    pub text: String,
    pub language: String,
    pub duration_seconds: f64,
    pub srt: String,
}

#[derive(Debug)]
pub struct AsrCpuStatus {
    pub available_cores: u32,
}

#[derive(Debug)]
pub enum AsrError {
    Network(String),
    Timeout,
    InvalidResponse(String),
    ServiceUnavailable,
}

impl std::fmt::Display for AsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AsrError::Network(e) => write!(f, "network error: {e}"),
            AsrError::Timeout => write!(f, "request timed out"),
            AsrError::InvalidResponse(e) => write!(f, "invalid response: {e}"),
            AsrError::ServiceUnavailable => write!(f, "ASR service unavailable"),
        }
    }
}

const SERVICE_URL: &str = "http://127.0.0.1:8765/transcribe";
const CPU_STATUS_URL: &str = "http://127.0.0.1:8765/cpu/status";
const TIMEOUT: Duration = Duration::from_secs(60);

pub async fn transcribe_voice(file_path: &Path, user_id: i64) -> Result<AsrResult, AsrError> {
    let audio_bytes = tokio::fs::read(file_path).await.map_err(|e| {
        AsrError::Network(format!("read file: {e}"))
    })?;

    let filename = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio.ogg")
        .to_string();

    let part = reqwest::multipart::Part::bytes(audio_bytes)
        .file_name(filename)
        .mime_str("application/octet-stream")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("audio", part)
        .text("user_id", user_id.to_string());

    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| AsrError::Network(e.to_string()))?;

    let response = client
        .post(SERVICE_URL)
        .multipart(form)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                AsrError::Timeout
            } else if e.is_connect() {
                AsrError::ServiceUnavailable
            } else {
                AsrError::Network(e.to_string())
            }
        })?;

    let status = response.status();
    if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
        return Err(AsrError::ServiceUnavailable);
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AsrError::InvalidResponse(format!("HTTP {status}: {body}")));
    }

    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AsrError::InvalidResponse(format!("JSON parse: {e}")))?;

    let text = json.get("text").and_then(|v| v.as_str()).ok_or_else(|| {
        AsrError::InvalidResponse("missing 'text' field".into())
    })?.to_string();

    let language = json
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let duration_seconds = json
        .get("duration_seconds")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let srt = json
        .get("srt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(AsrResult { text, language, duration_seconds, srt })
}

pub async fn get_cpu_status() -> Result<AsrCpuStatus, AsrError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| AsrError::Network(e.to_string()))?;

    let response = client
        .get(CPU_STATUS_URL)
        .send()
        .await
        .map_err(|e| AsrError::Network(e.to_string()))?;

    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AsrError::InvalidResponse(e.to_string()))?;

    let available_cores = json
        .get("available_cores")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    Ok(AsrCpuStatus { available_cores })
}
