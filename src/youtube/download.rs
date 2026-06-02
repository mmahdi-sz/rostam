use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{EditMessageTextParams, SendMessageParams, SendVideoParams},
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::i18n::{entities_for_text, t, tf};

use super::trace::log_trace;
use super::types::{VideoCodec, VideoFormatOption};

const DOWNLOAD_ROOT: &str = "/mnt/data/mahdidev/ros/dev/downloads/yt";
const PROGRESS_PREFIX: &str = "YT_PROGRESS|";
const EDIT_THROTTLE: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub struct YoutubeRequest {
    pub trace_id: u64,
    pub chat_id: i64,
    pub user_id: Option<i64>,
    pub webpage_url: String,
    pub cookie_spec: String,
    pub title: String,
    pub duration: Option<u64>,
    pub thumbnail_url: Option<String>,
    pub formats: Vec<VideoFormatOption>,
}

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static REQUESTS: OnceLock<Mutex<HashMap<u64, YoutubeRequest>>> = OnceLock::new();

fn store() -> &'static Mutex<HashMap<u64, YoutubeRequest>> {
    REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn store_request(req: YoutubeRequest) -> u64 {
    let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    log_trace(
        req.trace_id,
        "request_stored",
        &format!(
            "request_id={id} chat_id={} user_id={:?} formats={}",
            req.chat_id,
            req.user_id,
            req.formats.len()
        ),
    );
    store().lock().unwrap().insert(id, req);
    id
}

pub fn get_request(id: u64) -> Option<YoutubeRequest> {
    store().lock().unwrap().get(&id).cloned()
}

pub fn take_request(id: u64) -> Option<YoutubeRequest> {
    store().lock().unwrap().remove(&id)
}

pub fn codecs_for_height(req: &YoutubeRequest, height: u32) -> Vec<VideoCodec> {
    let mut out = Vec::new();
    for f in &req.formats {
        if f.height == height && !out.contains(&f.codec) {
            out.push(f.codec);
        }
    }
    out
}

fn find_format(req: &YoutubeRequest, height: u32, codec: VideoCodec) -> Option<&VideoFormatOption> {
    req.formats
        .iter()
        .find(|f| f.height == height && f.codec == codec)
}

fn build_bar(percent: f32) -> String {
    let total = 10usize;
    let filled = ((percent / 10.0).round() as i32).clamp(0, total as i32) as usize;
    let mut s = String::new();
    for _ in 0..filled {
        s.push('●');
    }
    for _ in 0..(total - filled) {
        s.push('○');
    }
    s
}

#[derive(Default, Clone)]
struct ProgressSnapshot {
    percent: String,
    downloaded: String,
    total: String,
    speed: String,
    eta: String,
    elapsed: String,
    percent_int: i32,
}

fn parse_progress_line(line: &str) -> Option<ProgressSnapshot> {
    let rest = line.strip_prefix(PROGRESS_PREFIX)?;
    let parts: Vec<&str> = rest.split('|').collect();
    if parts.len() < 6 {
        return None;
    }
    let percent_str = parts[0].trim().to_string();
    let percent_int = percent_str
        .trim_end_matches('%')
        .trim()
        .parse::<f32>()
        .ok()
        .map(|f| f.round() as i32)
        .unwrap_or(-1);
    Some(ProgressSnapshot {
        percent: percent_str,
        downloaded: parts[1].trim().to_string(),
        total: parts[2].trim().to_string(),
        speed: parts[3].trim().to_string(),
        eta: parts[4].trim().to_string(),
        elapsed: parts[5].trim().to_string(),
        percent_int,
    })
}

fn format_progress_body(snap: &ProgressSnapshot, quality_label: &str) -> String {
    let percent_f = snap
        .percent
        .trim_end_matches('%')
        .trim()
        .parse::<f32>()
        .unwrap_or(0.0);
    let bar = build_bar(percent_f);
    tf(
        "youtube.download.progress.body",
        &[
            ("quality", quality_label),
            ("percent", &snap.percent),
            ("bar", &bar),
            ("downloaded", &snap.downloaded),
            ("total", &snap.total),
            ("speed", &snap.speed),
            ("elapsed", &snap.elapsed),
            ("eta", &snap.eta),
        ],
    )
}

fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn format_upload_body(quality_label: &str, elapsed: Duration) -> String {
    tf(
        "youtube.download.progress.upload_body",
        &[
            ("quality", quality_label),
            ("elapsed", &format_elapsed(elapsed)),
        ],
    )
}

async fn edit_status(api: &Bot, chat_id: i64, message_id: i32, text: String) {
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    if let Err(error) = api.edit_message_text(&params).await {
        let desc = error.to_string();
        if !desc.contains("message is not modified") {
            eprintln!("[youtube event=edit_status_failed] {desc}");
        }
    }
}

pub fn spawn_download(
    api: Bot,
    request_id: u64,
    height: u32,
    codec: VideoCodec,
    status_chat_id: i64,
    status_message_id: i32,
) {
    tokio::spawn(async move {
        run_download(api, request_id, height, codec, status_chat_id, status_message_id).await
    });
}

async fn run_download(
    api: Bot,
    request_id: u64,
    height: u32,
    codec: VideoCodec,
    status_chat_id: i64,
    status_message_id: i32,
) {
    let Some(req) = take_request(request_id) else {
        let text = t("youtube.download.request_expired");
        edit_status(&api, status_chat_id, status_message_id, text).await;
        return;
    };
    let trace_id = req.trace_id;
    let quality_label = quality_label_for(height);
    log_trace(
        trace_id,
        "download_begin",
        &format!(
            "request_id={request_id} height={height} codec={} url={}",
            codec.key(),
            req.webpage_url
        ),
    );

    let Some(fmt) = find_format(&req, height, codec) else {
        log_trace(
            trace_id,
            "download_format_missing",
            &format!("height={height} codec={}", codec.key()),
        );
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            tf("youtube.download.failed", &[("error", "format not found")]),
        )
        .await;
        return;
    };
    let format_id = fmt.format_id.clone();

    let dir = PathBuf::from(format!("{DOWNLOAD_ROOT}/{trace_id}"));
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        log_trace(trace_id, "download_mkdir_failed", &e.to_string());
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            tf("youtube.download.failed", &[("error", &e.to_string())]),
        )
        .await;
        return;
    }

    let output_template = format!("{}/%(id)s.%(ext)s", dir.display());
    let format_spec = format!("{format_id}+bestaudio/best");
    let progress_template = format!(
        "{PROGRESS_PREFIX}%(progress._percent_str)s|%(progress._downloaded_bytes_str)s|%(progress._total_bytes_estimate_str)s|%(progress._speed_str)s|%(progress._eta_str)s|%(progress._elapsed_str)s"
    );

    log_trace(
        trace_id,
        "download_args",
        &format!(
            "cookie_spec={} format_spec={format_spec} output={output_template}",
            req.cookie_spec
        ),
    );

    let initial = ProgressSnapshot {
        percent: "0.0%".into(),
        downloaded: "0B".into(),
        total: "?".into(),
        speed: "?".into(),
        eta: "?".into(),
        elapsed: "00:00".into(),
        percent_int: 0,
    };
    edit_status(
        &api,
        status_chat_id,
        status_message_id,
        format_progress_body(&initial, &quality_label),
    )
    .await;

    let postprocess_template = format!(
        "{PROGRESS_PREFIX}%(progress._percent_str)s|%(progress._downloaded_bytes_str)s|%(progress._total_bytes_estimate_str)s|%(progress._speed_str)s|%(progress._eta_str)s|%(progress._elapsed_str)s"
    );
    let mut child = match Command::new("yt-dlp")
        .arg("--js-runtimes")
        .arg("deno:/root/.deno/bin/deno")
        .arg("--cookies-from-browser")
        .arg(&req.cookie_spec)
        .arg("--no-warnings")
        .arg("--no-playlist")
        .arg("--progress")
        .arg("--no-color")
        .arg("-f")
        .arg(&format_spec)
        .arg("--merge-output-format")
        .arg("mp4")
        .arg("--newline")
        .arg("--progress-template")
        .arg(format!("download:{progress_template}"))
        .arg("--progress-template")
        .arg(format!("postprocess:{postprocess_template}"))
        .arg("--print")
        .arg("after_move:filepath")
        .arg("-o")
        .arg(&output_template)
        .arg(&req.webpage_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log_trace(trace_id, "download_spawn_failed", &e.to_string());
            edit_status(
                &api,
                status_chat_id,
                status_message_id,
                tf("youtube.download.failed", &[("error", &e.to_string())]),
            )
            .await;
            return;
        }
    };

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let (tx, mut rx) = tokio::sync::mpsc::channel::<(&'static str, String)>(64);
    let tx_out = tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx_out.send(("stdout", line)).await;
        }
    });
    let tx_err = tx;
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx_err.send(("stderr", line)).await;
        }
    });

    let mut filepath: Option<String> = None;
    let mut last_edit = Instant::now() - EDIT_THROTTLE;
    let mut last_percent_int = -1;
    let mut stderr_tail = String::new();

    while let Some((source, line)) = rx.recv().await {
        if let Some(snap) = parse_progress_line(&line) {
            let now = Instant::now();
            if snap.percent_int != last_percent_int && now.duration_since(last_edit) >= EDIT_THROTTLE
            {
                last_percent_int = snap.percent_int;
                last_edit = now;
                log_trace(
                    trace_id,
                    "download_progress",
                    &format!(
                        "src={source} percent={} downloaded={} total={} speed={} eta={}",
                        snap.percent, snap.downloaded, snap.total, snap.speed, snap.eta
                    ),
                );
                edit_status(
                    &api,
                    status_chat_id,
                    status_message_id,
                    format_progress_body(&snap, &quality_label),
                )
                .await;
            }
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if source == "stdout"
            && trimmed.starts_with('/')
            && tokio::fs::metadata(trimmed).await.is_ok()
        {
            filepath = Some(trimmed.to_string());
            log_trace(trace_id, "download_filepath", trimmed);
        } else if source == "stderr" {
            stderr_tail = trimmed.to_string();
            log_trace(trace_id, "yt_dlp_stderr", trimmed);
        } else {
            log_trace(trace_id, "yt_dlp_stdout", trimmed);
        }
    }
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    let status = match child.wait().await {
        Ok(s) => s,
        Err(e) => {
            log_trace(trace_id, "download_wait_failed", &e.to_string());
            edit_status(
                &api,
                status_chat_id,
                status_message_id,
                tf("youtube.download.failed", &[("error", &e.to_string())]),
            )
            .await;
            return;
        }
    };

    if !status.success() {
        log_trace(
            trace_id,
            "download_failed",
            &format!("status={status} stderr_tail={stderr_tail}"),
        );
        let err = if stderr_tail.is_empty() {
            format!("exit {status}")
        } else {
            stderr_tail
        };
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            tf("youtube.download.failed", &[("error", &err)]),
        )
        .await;
        cleanup_dir(&dir, trace_id).await;
        return;
    }

    let path = match filepath.or_else(|| pick_largest_file(&dir)) {
        Some(p) => p,
        None => {
            log_trace(trace_id, "download_no_filepath", "no output file located");
            edit_status(
                &api,
                status_chat_id,
                status_message_id,
                tf("youtube.download.failed", &[("error", "no output file")]),
            )
            .await;
            cleanup_dir(&dir, trace_id).await;
            return;
        }
    };

    log_trace(
        trace_id,
        "download_complete",
        &format!("path={path} stderr_tail={stderr_tail}"),
    );
    edit_status(
        &api,
        status_chat_id,
        status_message_id,
        t("youtube.download.uploading"),
    )
    .await;

    let caption = tf(
        "youtube.download.caption",
        &[("title", &req.title), ("quality", &quality_label)],
    );
    let thumb_path = fetch_thumbnail(&req.thumbnail_url, &dir, trace_id).await;
    let mut params = SendVideoParams::builder()
        .chat_id(req.chat_id)
        .video(FileUpload::InputFile(InputFile {
            path: PathBuf::from(&path),
        }))
        .supports_streaming(true)
        .caption(caption)
        .build();
    if let Some(ref tp) = thumb_path {
        params.thumbnail = Some(FileUpload::InputFile(InputFile {
            path: PathBuf::from(tp),
        }));
    }
    if let Some(d) = req.duration {
        if d > 0 && d <= u32::MAX as u64 {
            params.duration = Some(d as u32);
        }
    }
    params.height = Some(height);

    log_trace(
        trace_id,
        "upload_start",
        &format!("path={path} thumb={}", thumb_path.as_deref().unwrap_or("none")),
    );

    let api_for_send = api.clone();
    let mut send_task = tokio::spawn(async move { api_for_send.send_video(&params).await });

    let upload_start = Instant::now();
    let mut interval = tokio::time::interval(EDIT_THROTTLE);
    interval.tick().await; // skip immediate first tick

    let send_result = loop {
        tokio::select! {
            result = &mut send_task => { break result; }
            _ = interval.tick() => {
                let elapsed = upload_start.elapsed();
                log_trace(trace_id, "upload_progress", &format!("elapsed={}s", elapsed.as_secs()));
                edit_status(
                    &api,
                    status_chat_id,
                    status_message_id,
                    format_upload_body(&quality_label, elapsed),
                )
                .await;
            }
        }
    };

    match send_result {
        Ok(Ok(_)) => {
            log_trace(trace_id, "upload_ok", &format!("path={path} elapsed={}s", upload_start.elapsed().as_secs()));
            let _ = api
                .delete_message(
                    &frankenstein::methods::DeleteMessageParams::builder()
                        .chat_id(status_chat_id)
                        .message_id(status_message_id)
                        .build(),
                )
                .await;
        }
        Ok(Err(e)) => {
            log_trace(trace_id, "upload_failed", &e.to_string());
            let _ = api
                .send_message(
                    &SendMessageParams::builder()
                        .chat_id(req.chat_id)
                        .text(tf(
                            "youtube.download.upload_failed",
                            &[("error", &e.to_string())],
                        ))
                        .build(),
                )
                .await;
        }
        Err(e) => {
            log_trace(trace_id, "upload_join_failed", &e.to_string());
        }
    }

    cleanup_dir(&dir, trace_id).await;
}

async fn fetch_thumbnail(
    url: &Option<String>,
    dir: &std::path::Path,
    trace_id: u64,
) -> Option<String> {
    let url = url.as_deref()?;
    let path = dir.join("thumb.jpg");
    match reqwest::get(url).await {
        Ok(resp) if resp.status().is_success() => {
            match resp.bytes().await {
                Ok(bytes) => {
                    if tokio::fs::write(&path, &bytes).await.is_ok() {
                        log_trace(
                            trace_id,
                            "thumb_fetched",
                            &format!("bytes={} path={}", bytes.len(), path.display()),
                        );
                        Some(path.to_string_lossy().into_owned())
                    } else {
                        log_trace(trace_id, "thumb_write_failed", url);
                        None
                    }
                }
                Err(e) => {
                    log_trace(trace_id, "thumb_bytes_failed", &e.to_string());
                    None
                }
            }
        }
        Ok(resp) => {
            log_trace(
                trace_id,
                "thumb_http_error",
                &format!("status={}", resp.status()),
            );
            None
        }
        Err(e) => {
            log_trace(trace_id, "thumb_fetch_failed", &e.to_string());
            None
        }
    }
}

fn pick_largest_file(dir: &std::path::Path) -> Option<String> {
    let mut best: Option<(u64, PathBuf)> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                let size = meta.len();
                if best.as_ref().map(|(s, _)| size > *s).unwrap_or(true) {
                    best = Some((size, entry.path()));
                }
            }
        }
    }
    best.map(|(_, p)| p.to_string_lossy().into_owned())
}

async fn cleanup_dir(dir: &std::path::Path, trace_id: u64) {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(_) => log_trace(trace_id, "cleanup_ok", &dir.display().to_string()),
        Err(e) => log_trace(trace_id, "cleanup_failed", &e.to_string()),
    }
}

fn quality_label_for(height: u32) -> String {
    let key = format!("youtube.quality.buttons.{height}");
    let label = t(&key);
    if label.starts_with('!') {
        format!("{height}p")
    } else {
        label
    }
}
