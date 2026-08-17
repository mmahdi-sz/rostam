//! Byte-accurate transfer metering for Telegram uploads and downloads.
//! Speed and ETA come from counted bytes, never from an assumed total.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::TryStreamExt;
use reqwest::multipart;
use tokio::sync::OnceCell;
use tokio_util::io::ReaderStream;

use crate::i18n;

// Re-use components
pub use crate::youtube::download::progress::{build_bar, format_elapsed};

const WINDOW: Duration = Duration::from_secs(3);
const MIN_SPAN: Duration = Duration::from_millis(700);
const CHUNK: usize = 64 * 1024;

/// Which leg is running. Speed is only meaningful where bytes actually move.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    /// Local Bot API is pulling the file from Telegram (getFile) — no observable bytes.
    Fetching = 0,
    /// Bytes moving through our socket (upload, or HTTP download).
    Streaming = 1,
    /// Disk-to-disk copy from the local Bot API storage dir — not network speed.
    Copying = 2,
    /// Body fully sent; the Bot API server is forwarding to Telegram — no observable bytes.
    Finalizing = 3,
    Done = 4,
}

impl Stage {
    fn as_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(val: u8) -> Self {
        match val {
            0 => Stage::Fetching,
            1 => Stage::Streaming,
            2 => Stage::Copying,
            3 => Stage::Finalizing,
            _ => Stage::Done,
        }
    }
}

pub struct TransferSnapshot {
    pub stage: String,
    pub percent: String,
    pub bar: String,
    pub done: String,
    pub total: String,
    pub speed: String,
    pub eta: String,
    pub elapsed: String,
}

pub struct TransferProgress {
    total: AtomicU64,
    done: AtomicU64,
    stage: AtomicU8,
    started: Instant,
    leg_started: Mutex<Instant>,
    window: Mutex<VecDeque<(Instant, u64)>>,
    finalize_bps_hint: AtomicU64,
}

impl TransferProgress {
    pub fn new(total: u64) -> Arc<Self> {
        Arc::new(Self {
            total: AtomicU64::new(total),
            done: AtomicU64::new(0),
            stage: AtomicU8::new(Stage::Fetching.as_u8()),
            started: Instant::now(),
            leg_started: Mutex::new(Instant::now()),
            window: Mutex::new(VecDeque::new()),
            finalize_bps_hint: AtomicU64::new(0),
        })
    }

    pub fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
    }

    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    pub fn bump(&self, n: u64) {
        let current_done = self.done.fetch_add(n, Ordering::Relaxed) + n;
        let now = Instant::now();
        let mut w = self.window.lock().unwrap();
        w.push_back((now, current_done));
        while let Some(&(t, _)) = w.front() {
            if now.duration_since(t) > WINDOW {
                w.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn set_stage(&self, stage: Stage) {
        self.stage.store(stage.as_u8(), Ordering::Relaxed);
        *self.leg_started.lock().unwrap() = Instant::now();
        self.window.lock().unwrap().clear();
    }

    pub fn stage(&self) -> Stage {
        Stage::from_u8(self.stage.load(Ordering::Relaxed))
    }

    pub fn speed_bps(&self) -> Option<f64> {
        let st = self.stage();
        if st == Stage::Done {
            let done = self.done();
            let elapsed = self.elapsed();
            if elapsed.as_secs_f64() > 0.0 {
                return Some(done as f64 / elapsed.as_secs_f64());
            } else {
                return None;
            }
        }
        if st != Stage::Streaming && st != Stage::Copying {
            return None;
        }
        let w = self.window.lock().unwrap();
        if w.len() < 2 {
            return None;
        }
        let (first_t, first_done) = *w.front().unwrap();
        let (last_t, last_done) = *w.back().unwrap();
        let span = last_t.duration_since(first_t);
        if span < MIN_SPAN {
            return None;
        }
        let bytes = last_done.saturating_sub(first_done);
        Some(bytes as f64 / span.as_secs_f64())
    }

    pub fn eta(&self) -> Option<Duration> {
        let total = self.total();
        if total == 0 {
            return None;
        }
        let st = self.stage();
        if st == Stage::Streaming || st == Stage::Copying {
            let speed = self.speed_bps()?;
            if speed > 0.0 {
                let remaining = total.saturating_sub(self.done()) as f64;
                Some(Duration::from_secs_f64(remaining / speed))
            } else {
                None
            }
        } else if st == Stage::Fetching || st == Stage::Finalizing {
            let hint = self.finalize_bps_hint.load(Ordering::Relaxed);
            if hint == 0 {
                return None;
            }
            let total_f = total as f64;
            let elapsed = self.leg_elapsed().as_secs_f64();
            let estimated_total_time = total_f / (hint as f64);
            let remaining = estimated_total_time - elapsed;
            if remaining > 0.0 {
                Some(Duration::from_secs_f64(remaining))
            } else {
                Some(Duration::ZERO)
            }
        } else {
            None
        }
    }

    pub fn percent(&self) -> Option<f64> {
        let total = self.total();
        if total == 0 {
            None
        } else {
            let done = self.done();
            if done >= total {
                Some(100.0)
            } else {
                Some((done as f64 / total as f64) * 100.0)
            }
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn leg_elapsed(&self) -> Duration {
        self.leg_started.lock().unwrap().elapsed()
    }

    #[allow(dead_code)]
    pub fn is_complete(&self) -> bool {
        let total = self.total();
        total > 0 && self.done() >= total
    }

    pub fn snapshot(&self) -> TransferSnapshot {
        let stage_str = match self.stage() {
            Stage::Fetching => i18n::t("transfer.stage.fetching"),
            Stage::Copying => i18n::t("transfer.stage.copying"),
            Stage::Streaming => i18n::t("transfer.stage.uploading"),
            Stage::Finalizing => i18n::t("transfer.stage.finalizing"),
            Stage::Done => {
                let total = self.total();
                let elapsed = self.elapsed();
                let speed_bps = if elapsed.as_secs_f64() > 0.0 {
                    (total as f64) / elapsed.as_secs_f64()
                } else {
                    0.0
                };
                return TransferSnapshot {
                    stage: i18n::tf(
                        "transfer.done",
                        &[
                            ("size", &fmt_bytes(total)),
                            ("elapsed", &format_elapsed(elapsed)),
                            ("speed", &fmt_speed(speed_bps)),
                        ],
                    ),
                    percent: "100%".to_string(),
                    bar: build_bar(100.0),
                    done: fmt_bytes(total),
                    total: fmt_bytes(total),
                    speed: fmt_speed(speed_bps),
                    eta: if total == 0 {
                        "—".to_string()
                    } else {
                        "00:00".to_string()
                    },
                    elapsed: format_elapsed(elapsed),
                };
            }
        };

        let pct = self.percent();
        let speed = self.speed_bps();
        let eta = self.eta();
        let total = self.total();
        let done = self.done();

        TransferSnapshot {
            stage: stage_str,
            percent: pct
                .map(|p| format!("{:.1}%", p))
                .unwrap_or_else(|| "".to_string()),
            bar: build_bar(pct.unwrap_or(0.0) as f32),
            done: fmt_bytes(done),
            total: if total == 0 {
                "—".to_string()
            } else {
                fmt_bytes(total)
            },
            speed: speed.map(fmt_speed).unwrap_or_else(|| "—".to_string()),
            eta: eta.map(format_elapsed).unwrap_or_else(|| "—".to_string()),
            elapsed: format_elapsed(self.elapsed()),
        }
    }

    pub fn set_finalize_bps_hint(&self, hint: u64) {
        self.finalize_bps_hint.store(hint, Ordering::Relaxed);
    }
}

pub fn fmt_speed(bps: f64) -> String {
    if bps >= 1_000_000.0 {
        format!("{:.1} MB/s", bps / 1_000_000.0)
    } else if bps >= 1_000.0 {
        format!("{:.0} KB/s", bps / 1_000.0)
    } else {
        format!("{:.0} B/s", bps)
    }
}

pub fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1_000_000_000.0 {
        format!("{:.2} GB", b / 1_000_000_000.0)
    } else if b >= 1_000_000.0 {
        format!("{:.1} MB", b / 1_000_000.0)
    } else if b >= 1_000.0 {
        format!("{:.0} KB", b / 1_000.0)
    } else {
        format!("{:.0} B", b)
    }
}

static REDIS_CLIENT: OnceCell<redis::Client> = OnceCell::const_new();

async fn get_redis_conn() -> Option<redis::aio::MultiplexedConnection> {
    let client = REDIS_CLIENT
        .get_or_init(|| async {
            let url =
                std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1/".to_string());
            redis::Client::open(url).expect("Invalid Redis URL")
        })
        .await;
    client.get_multiplexed_async_connection().await.ok()
}

pub async fn load_ema_bps(key: &str) -> Option<u64> {
    let mut conn = get_redis_conn().await?;
    let val: Option<u64> = redis::cmd("GET")
        .arg(key)
        .query_async(&mut conn)
        .await
        .ok()?;
    val
}

pub async fn update_ema_bps(key: &str, sample_bps: u64) {
    if sample_bps == 0 {
        return;
    }
    if let Some(mut conn) = get_redis_conn().await {
        let old: Option<u64> = redis::cmd("GET")
            .arg(key)
            .query_async(&mut conn)
            .await
            .unwrap_or(None);
        let new_val = if let Some(old_val) = old {
            ((0.3 * sample_bps as f64) + (0.7 * old_val as f64)) as u64
        } else {
            sample_bps
        };
        let _: Option<()> = redis::cmd("SET")
            .arg(key)
            .arg(new_val)
            .arg("EX")
            .arg(30 * 24 * 3600)
            .query_async(&mut conn)
            .await
            .ok();
    }
}

fn ema_key_fetch() -> String {
    format!("transfer:ema:fetch_bps:{}", crate::config::env_label())
}

fn ema_key_finalize() -> String {
    format!("transfer:ema:finalize_bps:{}", crate::config::env_label())
}

pub async fn record_fetch_sample(total_bytes: u64, elapsed: Duration) {
    let secs = elapsed.as_secs_f64();
    if secs > 0.0 {
        update_ema_bps(&ema_key_fetch(), (total_bytes as f64 / secs) as u64).await;
    }
}

pub async fn record_finalize_sample(total_bytes: u64, elapsed: Duration) {
    let secs = elapsed.as_secs_f64();
    if secs > 0.0 {
        update_ema_bps(&ema_key_finalize(), (total_bytes as f64 / secs) as u64).await;
    }
}

pub async fn send_params_metered<P, R>(
    api_url: &str,
    method: &str,
    params: &P,
    progress: &Arc<TransferProgress>,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> anyhow::Result<R>
where
    P: serde::Serialize,
    R: serde::de::DeserializeOwned,
{
    if let Some(hint) = load_ema_bps(&ema_key_finalize()).await {
        progress.set_finalize_bps_hint(hint);
    }

    let val = serde_json::to_value(params)?;
    let map = val
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("params is not an object"))?;

    let file_allowlist = [
        "photo",
        "audio",
        "document",
        "video",
        "animation",
        "voice",
        "video_note",
        "thumbnail",
        "cover",
        "sticker",
        "media",
    ];

    let mut form = multipart::Form::new();
    let mut files_to_send = Vec::new();
    let mut total_bytes = 0;

    for (k, v) in map {
        if file_allowlist.contains(&k.as_str()) {
            if let Some(obj) = v.as_object() {
                if obj.len() == 1 && obj.contains_key("path") {
                    if let Some(path_val) = obj.get("path") {
                        if let Some(path_str) = path_val.as_str() {
                            let path = std::path::Path::new(path_str);
                            if let Ok(meta) = tokio::fs::metadata(path).await {
                                total_bytes += meta.len();
                                files_to_send.push((k.clone(), path_str.to_string(), meta.len()));
                                continue;
                            }
                        }
                    }
                }
            }
        }

        let field_val = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        form = form.text(k.clone(), field_val);
    }

    progress.set_total(total_bytes);

    for (param_name, path_str, len) in files_to_send {
        let f = tokio::fs::File::open(&path_str).await?;
        let p = progress.clone();
        let cancel_clone = cancel.clone();

        let stream = ReaderStream::with_capacity(f, CHUNK)
            .map_err(|e| e)
            .and_then(move |c| {
                if let Some(cancel) = &cancel_clone {
                    if cancel.load(Ordering::Relaxed) {
                        return futures::future::ready(Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            "cancelled",
                        )));
                    }
                }
                futures::future::ready(Ok(c))
            })
            .inspect_ok(move |c| p.bump(c.len() as u64));

        let file_name = std::path::Path::new(&path_str)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();
        let part = multipart::Part::stream_with_length(reqwest::Body::wrap_stream(stream), len)
            .file_name(file_name);

        form = form.part(param_name, part);
    }

    progress.set_stage(Stage::Streaming);

    let client = crate::http::client();
    let url = format!("{}/{}", api_url.trim_end_matches('/'), method);

    let resp = client.post(&url).multipart(form).send().await?;

    let finalize_start = Instant::now();
    progress.set_stage(Stage::Finalizing);

    let status = resp.status();
    let body = resp.text().await?;

    let finalize_elapsed = finalize_start.elapsed();

    if !status.is_success() {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(desc) = json.get("description").and_then(|v| v.as_str()) {
                return Err(anyhow::anyhow!("Telegram error: {}", desc));
            }
        }
        return Err(anyhow::anyhow!("HTTP error {}: {}", status, body));
    }

    progress.set_stage(Stage::Done);
    record_finalize_sample(total_bytes, finalize_elapsed).await;

    if let Ok(result) = serde_json::from_str::<R>(&body) {
        return Ok(result);
    }
    if let Ok(wrapped) = serde_json::from_str::<frankenstein::response::MethodResponse<R>>(&body) {
        return Ok(wrapped.result);
    }

    let result: R = serde_json::from_str(&body)?;
    Ok(result)
}

pub async fn send_file_with_upload_ticker<
    P: serde::Serialize + Clone + Send + Sync + 'static,
    R: serde::de::DeserializeOwned + Send + 'static,
>(
    api: &frankenstein::client_reqwest::Bot,
    method: &str,
    params: &P,
    file_path: &std::path::Path,
    status_chat_id: i64,
    status_message_id: i32,
    stage_key: &str,
    cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> anyhow::Result<R> {
    if status_message_id == 0 {
        return send_params_metered::<P, R>(
            &api.api_url,
            method,
            params,
            &TransferProgress::new(0),
            cancel_flag,
        )
        .await;
    }
    let file_bytes = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);
    let progress = TransferProgress::new(file_bytes);
    let progress_clone = progress.clone();
    let api_url = api.api_url.clone();
    let cancel_clone = cancel_flag.clone();
    let method_str = method.to_string();

    let params_owned = (*params).clone();
    let mut send_task = tokio::spawn(async move {
        send_params_metered::<P, R>(
            &api_url,
            &method_str,
            &params_owned,
            &progress_clone,
            cancel_clone,
        )
        .await
    });

    let mut interval = tokio::time::interval(Duration::from_secs(2));
    interval.tick().await;
    let mut last_body = String::new();

    loop {
        tokio::select! {
            res = &mut send_task => {
                return match res {
                    Ok(inner) => inner,
                    Err(e) => Err(anyhow::anyhow!("Upload task join error: {e}")),
                };
            }
            _ = interval.tick() => {
                let snap = progress.snapshot();
                let stage_label = i18n::t(stage_key);
                let eta_label = i18n::t("transfer.eta_label");
                let body = format!(
                    "⬆️ {stage_label}\n[{}] {}\n📦 {} / {} | 🚀 {}\n⏱ {eta_label}: {}",
                    snap.bar,
                    snap.percent,
                    snap.done,
                    snap.total,
                    snap.speed,
                    snap.eta
                );
                if body != last_body {
                    last_body = body.clone();
                    let _ = crate::bot::edit_text(api, status_chat_id, status_message_id, &body, None).await;
                }
            }
        }
    }
}

#[allow(dead_code)]
pub trait AsyncTelegramApiMetered {
    async fn send_document_metered(
        &self,
        params: &frankenstein::methods::SendDocumentParams,
    ) -> anyhow::Result<frankenstein::types::Message>;

    async fn send_video_metered(
        &self,
        params: &frankenstein::methods::SendVideoParams,
    ) -> anyhow::Result<frankenstein::types::Message>;

    async fn send_audio_metered(
        &self,
        params: &frankenstein::methods::SendAudioParams,
    ) -> anyhow::Result<frankenstein::types::Message>;

    async fn send_voice_metered(
        &self,
        params: &frankenstein::methods::SendVoiceParams,
    ) -> anyhow::Result<frankenstein::types::Message>;

    async fn send_photo_metered(
        &self,
        params: &frankenstein::methods::SendPhotoParams,
    ) -> anyhow::Result<frankenstein::types::Message>;
}

impl AsyncTelegramApiMetered for frankenstein::client_reqwest::Bot {
    async fn send_document_metered(
        &self,
        params: &frankenstein::methods::SendDocumentParams,
    ) -> anyhow::Result<frankenstein::types::Message> {
        let progress = TransferProgress::new(0);
        let resp: frankenstein::response::MethodResponse<frankenstein::types::Message> =
            send_params_metered(
                self.api_url.as_str(),
                "sendDocument",
                params,
                &progress,
                None,
            )
            .await?;
        Ok(resp.result)
    }

    async fn send_video_metered(
        &self,
        params: &frankenstein::methods::SendVideoParams,
    ) -> anyhow::Result<frankenstein::types::Message> {
        let progress = TransferProgress::new(0);
        let resp: frankenstein::response::MethodResponse<frankenstein::types::Message> =
            send_params_metered(self.api_url.as_str(), "sendVideo", params, &progress, None)
                .await?;
        Ok(resp.result)
    }

    async fn send_audio_metered(
        &self,
        params: &frankenstein::methods::SendAudioParams,
    ) -> anyhow::Result<frankenstein::types::Message> {
        let progress = TransferProgress::new(0);
        let resp: frankenstein::response::MethodResponse<frankenstein::types::Message> =
            send_params_metered(self.api_url.as_str(), "sendAudio", params, &progress, None)
                .await?;
        Ok(resp.result)
    }

    async fn send_voice_metered(
        &self,
        params: &frankenstein::methods::SendVoiceParams,
    ) -> anyhow::Result<frankenstein::types::Message> {
        let progress = TransferProgress::new(0);
        let resp: frankenstein::response::MethodResponse<frankenstein::types::Message> =
            send_params_metered(self.api_url.as_str(), "sendVoice", params, &progress, None)
                .await?;
        Ok(resp.result)
    }

    async fn send_photo_metered(
        &self,
        params: &frankenstein::methods::SendPhotoParams,
    ) -> anyhow::Result<frankenstein::types::Message> {
        let progress = TransferProgress::new(0);
        let resp: frankenstein::response::MethodResponse<frankenstein::types::Message> =
            send_params_metered(self.api_url.as_str(), "sendPhoto", params, &progress, None)
                .await?;
        Ok(resp.result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frankenstein::input_file::{FileUpload, InputFile};
    use frankenstein::methods::SendVideoParams;

    #[test]
    fn test_zero_total() {
        let p = TransferProgress::new(0);
        assert_eq!(p.percent(), None);
        assert_eq!(p.eta(), None);
    }

    #[test]
    fn test_fmt_speed() {
        assert_eq!(fmt_speed(999.0), "999 B/s");
        assert_eq!(fmt_speed(1000.0), "1 KB/s");
        assert_eq!(fmt_speed(999_999.0), "1000 KB/s");
        assert_eq!(fmt_speed(1_000_000.0), "1.0 MB/s");
    }

    #[test]
    fn test_fmt_bytes() {
        assert_eq!(fmt_bytes(999), "999 B");
        assert_eq!(fmt_bytes(1000), "1 KB");
        assert_eq!(fmt_bytes(1_000_000), "1.0 MB");
        assert_eq!(fmt_bytes(1_000_000_000), "1.00 GB");
    }

    #[test]
    fn test_detect_files() {
        let params = SendVideoParams::builder()
            .chat_id(123)
            .video(FileUpload::InputFile(InputFile {
                path: "/path/to/video.mp4".into(),
            }))
            .thumbnail(FileUpload::InputFile(InputFile {
                path: "/path/to/thumb.jpg".into(),
            }))
            .caption("test caption".to_string())
            .build();

        let val = serde_json::to_value(&params).unwrap();
        let map = val.as_object().unwrap();

        let file_allowlist = ["video", "thumbnail"];

        let mut count = 0;
        for (k, v) in map {
            if file_allowlist.contains(&k.as_str()) {
                if let Some(obj) = v.as_object() {
                    if obj.len() == 1 && obj.contains_key("path") {
                        count += 1;
                    }
                }
            }
        }

        assert_eq!(count, 2);
    }
}
