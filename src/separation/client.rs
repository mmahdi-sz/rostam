use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::error::SeparationError;
use super::types::{SeparationMode, SeparationResult};

static NEXT_TRACE: AtomicU64 = AtomicU64::new(1);

fn next_trace_id() -> u64 {
    NEXT_TRACE.fetch_add(1, Ordering::Relaxed)
}

const SERVICE_URL: &str = "http://127.0.0.1:6589/separate";
const TIMEOUT: Duration = Duration::from_secs(600); // 10 minutes

pub async fn separate_audio(
    audio_bytes: Vec<u8>,
    filename: &str,
    mode: SeparationMode,
    user_id: i64,
) -> Result<SeparationResult, SeparationError> {
    let trace_id = next_trace_id();
    let file_size = audio_bytes.len();
    let mode_str = mode.as_str();

    eprintln!("[separation trace={trace_id} event=request_start] user_id={user_id} mode={mode_str} file_size_bytes={file_size}");

    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| {
            eprintln!("[separation trace={trace_id} event=error] type=client_build err={e}");
            SeparationError::ServiceUnavailable
        })?;

    let part = reqwest::multipart::Part::bytes(audio_bytes)
        .file_name(filename.to_string())
        .mime_str("application/octet-stream")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("mode", mode_str.to_string());

    eprintln!("[separation trace={trace_id} event=service_post] url={SERVICE_URL} timeout=600s");

    let t_start = std::time::Instant::now();
    let response = match client.post(SERVICE_URL).multipart(form).send().await {
        Ok(r) => r,
        Err(e) => {
            let elapsed_ms = t_start.elapsed().as_millis();
            if e.is_timeout() {
                eprintln!("[separation trace={trace_id} event=error] type=timeout elapsed_ms={elapsed_ms}");
                return Err(SeparationError::Timeout);
            }
            if e.is_connect() {
                eprintln!("[separation trace={trace_id} event=error] type=service_unavailable err={e} elapsed_ms={elapsed_ms}");
                return Err(SeparationError::ServiceUnavailable);
            }
            eprintln!("[separation trace={trace_id} event=error] type=http_send err={e} elapsed_ms={elapsed_ms}");
            return Err(SeparationError::ProcessingFailed(e.to_string()));
        }
    };

    let elapsed_ms = t_start.elapsed().as_millis();
    let status = response.status();
    eprintln!("[separation trace={trace_id} event=service_response] status={status} duration_ms={elapsed_ms}");

    if status == reqwest::StatusCode::SERVICE_UNAVAILABLE || status == reqwest::StatusCode::BAD_GATEWAY {
        eprintln!("[separation trace={trace_id} event=error] type=service_unavailable status={status}");
        return Err(SeparationError::ServiceUnavailable);
    }
    if status == reqwest::StatusCode::BAD_REQUEST {
        let body = response.text().await.unwrap_or_default();
        eprintln!("[separation trace={trace_id} event=error] type=invalid_audio body={body}");
        return Err(SeparationError::InvalidAudio);
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        eprintln!("[separation trace={trace_id} event=error] type=processing_failed status={status} body={body}");
        return Err(SeparationError::ProcessingFailed(format!("HTTP {status}: {body}")));
    }

    let json: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[separation trace={trace_id} event=error] type=json_parse err={e}");
            return Err(SeparationError::ProcessingFailed(format!("JSON parse: {e}")));
        }
    };

    if let Some(err_msg) = json.get("error").and_then(|v| v.as_str()) {
        eprintln!("[separation trace={trace_id} event=error] type=service_error msg={err_msg}");
        return Err(SeparationError::ProcessingFailed(err_msg.to_string()));
    }

    let vocals_b64 = json.get("vocals").and_then(|v| v.as_str()).ok_or_else(|| {
        eprintln!("[separation trace={trace_id} event=error] type=missing_field field=vocals");
        SeparationError::ProcessingFailed("missing vocals field".into())
    })?;
    let instrumental_b64 = json.get("instrumental").and_then(|v| v.as_str()).ok_or_else(|| {
        eprintln!("[separation trace={trace_id} event=error] type=missing_field field=instrumental");
        SeparationError::ProcessingFailed("missing instrumental field".into())
    })?;
    let duration_seconds = json.get("duration_seconds").and_then(|v| v.as_f64()).unwrap_or(0.0);

    let vocals_wav = b64_decode(vocals_b64).map_err(|e| {
        eprintln!("[separation trace={trace_id} event=error] type=base64_vocals err={e}");
        SeparationError::ProcessingFailed(format!("base64 vocals: {e}"))
    })?;
    let instrumental_wav = b64_decode(instrumental_b64).map_err(|e| {
        eprintln!("[separation trace={trace_id} event=error] type=base64_instrumental err={e}");
        SeparationError::ProcessingFailed(format!("base64 instrumental: {e}"))
    })?;

    eprintln!(
        "[separation trace={trace_id} event=decode_complete] vocals_size={} instrumental_size={} audio_duration={duration_seconds:.1}s",
        vocals_wav.len(), instrumental_wav.len()
    );

    Ok(SeparationResult { vocals_wav, instrumental_wav, duration_seconds })
}

// Simple RFC-4648 base64 decoder — no external crate needed.
fn b64_decode(s: &str) -> Result<Vec<u8>, &'static str> {
    const TABLE: [i8; 256] = {
        let mut t = [-1i8; 256];
        let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0usize;
        while i < 64 {
            t[chars[i] as usize] = i as i8;
            i += 1;
        }
        t
    };

    let s = s.trim().as_bytes();
    let mut out = Vec::with_capacity(s.len() * 3 / 4 + 3);
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'=' { break; }
        let a = TABLE[s[i] as usize];
        if a < 0 { return Err("invalid base64 char"); }
        let b_val = if i + 1 < s.len() { TABLE[s[i+1] as usize] } else { return Err("truncated"); };
        if b_val < 0 { return Err("invalid base64 char"); }
        out.push((a as u8) << 2 | (b_val as u8) >> 4);
        if i + 2 >= s.len() || s[i+2] == b'=' { i += 4; break; }
        let c_val = TABLE[s[i+2] as usize];
        if c_val < 0 { return Err("invalid base64 char"); }
        out.push((b_val as u8) << 4 | (c_val as u8) >> 2);
        if i + 3 >= s.len() || s[i+3] == b'=' { i += 4; break; }
        let d_val = TABLE[s[i+3] as usize];
        if d_val < 0 { return Err("invalid base64 char"); }
        out.push((c_val as u8) << 6 | d_val as u8);
        i += 4;
    }
    Ok(out)
}
