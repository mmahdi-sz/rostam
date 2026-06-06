use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{EditMessageTextParams, SendMessageParams, SendVideoParams},
    types::{ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup},
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Notify;

use crate::i18n::{entities_for_text, t, tf};

use super::trace::log_trace;
use super::types::{AudioLanguage, SubtitleLanguage, VideoCodec, VideoFormatOption};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SubtitleMode {
    File,
    Embedded,
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub height: u32,
    pub codec: VideoCodec,
    pub audio_lang: Option<String>,
    pub subtitle_langs: Vec<String>,
    pub subtitle_mode: SubtitleMode,
    pub view: SelectionView,
}

#[derive(Clone, Copy, Debug)]
pub enum SelectionView {
    Main,
    SubMenu(usize),
}

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
    pub audio_languages: Vec<AudioLanguage>,
    pub subtitle_languages: Vec<SubtitleLanguage>,
    pub selection: Arc<Mutex<Option<Selection>>>,
}

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static REQUESTS: OnceLock<Mutex<HashMap<u64, YoutubeRequest>>> = OnceLock::new();
static ACTIVE_DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<Notify>>>> = OnceLock::new();

fn store() -> &'static Mutex<HashMap<u64, YoutubeRequest>> {
    REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn active_downloads() -> &'static Mutex<HashMap<u64, Arc<Notify>>> {
    ACTIVE_DOWNLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_cancel(request_id: u64) -> Arc<Notify> {
    let notify = Arc::new(Notify::new());
    active_downloads().lock().unwrap().insert(request_id, notify.clone());
    notify
}

fn unregister_cancel(request_id: u64) {
    active_downloads().lock().unwrap().remove(&request_id);
}

pub fn cancel_download(request_id: u64) -> bool {
    if let Some(notify) = active_downloads().lock().unwrap().remove(&request_id) {
        notify.notify_one();
        true
    } else {
        false
    }
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

const CODEC_PRIORITY: &[VideoCodec] = &[
    VideoCodec::Av1,
    VideoCodec::Vp9,
    VideoCodec::H265,
    VideoCodec::H264,
];

pub fn pick_default_codec(available: &[VideoCodec]) -> VideoCodec {
    for c in CODEC_PRIORITY {
        if available.contains(c) {
            return *c;
        }
    }
    available.first().copied().unwrap_or(VideoCodec::H264)
}

pub fn pick_default_audio(langs: &[AudioLanguage]) -> Option<String> {
    if let Some(original) = langs.iter().find(|l| l.is_original) {
        return Some(original.code.clone());
    }
    langs.first().map(|l| l.code.clone())
}

pub fn init_selection(req: &YoutubeRequest, height: u32) -> Selection {
    let codecs = codecs_for_height(req, height);
    let codec = pick_default_codec(&codecs);
    let audio_lang = pick_default_audio(&req.audio_languages);
    Selection {
        height,
        codec,
        audio_lang,
        subtitle_langs: Vec::new(),
        subtitle_mode: SubtitleMode::Embedded,
        view: SelectionView::Main,
    }
}

pub fn with_selection<F, R>(req: &YoutubeRequest, f: F) -> R
where
    F: FnOnce(&mut Option<Selection>) -> R,
{
    let mut guard = req.selection.lock().unwrap();
    f(&mut *guard)
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
    if parts.len() < 7 {
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
    let total = {
        let exact = parts[2].trim();
        if exact.is_empty() || exact == "N/A" {
            parts[3].trim().to_string()
        } else {
            exact.to_string()
        }
    };
    Some(ProgressSnapshot {
        percent: percent_str,
        downloaded: parts[1].trim().to_string(),
        total,
        speed: parts[4].trim().to_string(),
        eta: parts[5].trim().to_string(),
        elapsed: parts[6].trim().to_string(),
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

const CB_CANCEL_PREFIX: &str = "yt:cancel:";

fn cancel_keyboard(request_id: u64) -> InlineKeyboardMarkup {
    let button = InlineKeyboardButton {
        text: t("youtube.download.cancel_button"),
        callback_data: Some(format!("{CB_CANCEL_PREFIX}{request_id}")),
        style: Some(ButtonStyle::Danger),
        icon_custom_emoji_id: None,
        url: None,
        login_url: None,
        web_app: None,
        switch_inline_query: None,
        switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None,
        copy_text: None,
        callback_game: None,
        pay: None,
    };
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![button]])
        .build()
}

async fn edit_progress_status(api: &Bot, chat_id: i64, message_id: i32, text: String, request_id: u64) {
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .reply_markup(cancel_keyboard(request_id))
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    if let Err(error) = api.edit_message_text(&params).await {
        let desc = error.to_string();
        if !desc.contains("message is not modified") {
            eprintln!("[youtube event=edit_progress_status_failed] {desc}");
        }
    }
}

struct UnregisterGuard(u64);
impl Drop for UnregisterGuard {
    fn drop(&mut self) {
        unregister_cancel(self.0);
    }
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
    selection: Selection,
    status_chat_id: i64,
    status_message_id: i32,
) {
    let cancel = register_cancel(request_id);
    tokio::spawn(async move {
        run_download(api, request_id, selection, status_chat_id, status_message_id, cancel).await
    });
}

async fn run_download(
    api: Bot,
    request_id: u64,
    selection: Selection,
    status_chat_id: i64,
    status_message_id: i32,
    cancel: Arc<Notify>,
) {
    let height = selection.height;
    let codec = selection.codec;
    let Some(req) = take_request(request_id) else {
        let text = t("youtube.download.request_expired");
        edit_status(&api, status_chat_id, status_message_id, text).await;
        unregister_cancel(request_id);
        return;
    };
    let _cancel_guard = UnregisterGuard(request_id);
    let cancel_fut = std::pin::pin!(cancel.notified());
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
        "{PROGRESS_PREFIX}%(progress._percent_str)s|%(progress._downloaded_bytes_str)s|%(progress._total_bytes_str)s|%(progress._total_bytes_estimate_str)s|%(progress._speed_str)s|%(progress._eta_str)s|%(progress._elapsed_str)s"
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
    edit_progress_status(
        &api,
        status_chat_id,
        status_message_id,
        format_progress_body(&initial, &quality_label),
        request_id,
    )
    .await;

    let postprocess_template = format!(
        "{PROGRESS_PREFIX}%(progress._percent_str)s|%(progress._downloaded_bytes_str)s|%(progress._total_bytes_str)s|%(progress._total_bytes_estimate_str)s|%(progress._speed_str)s|%(progress._eta_str)s|%(progress._elapsed_str)s"
    );
    let mut cmd = Command::new("yt-dlp");
    cmd.arg("--js-runtimes")
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
        .arg(&output_template);

    if !selection.subtitle_langs.is_empty() {
        let sub_langs = selection.subtitle_langs.join(",");
        cmd.arg("--write-subs")
            .arg("--sub-langs")
            .arg(&sub_langs);
        if selection.subtitle_mode == SubtitleMode::Embedded {
            cmd.arg("--embed-subs");
        }
        log_trace(
            trace_id,
            "download_subtitle_args",
            &format!(
                "sub_langs={sub_langs} mode={:?}",
                selection.subtitle_mode
            ),
        );
    }

    cmd.arg(&req.webpage_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn()
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
    let mut cancel_fut = cancel_fut;

    loop {
        tokio::select! {
            msg = rx.recv() => {
                let Some((source, line)) = msg else { break; };
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
                        edit_progress_status(
                            &api,
                            status_chat_id,
                            status_message_id,
                            format_progress_body(&snap, &quality_label),
                            request_id,
                        )
                        .await;
                    }
                    continue;
                }
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                if source == "stdout"
                    && trimmed.starts_with('/')
                    && tokio::fs::metadata(&trimmed).await.is_ok()
                {
                    filepath = Some(trimmed.clone());
                    log_trace(trace_id, "download_filepath", &trimmed);
                } else if source == "stderr" {
                    stderr_tail = trimmed.clone();
                    log_trace(trace_id, "yt_dlp_stderr", &trimmed);
                } else {
                    log_trace(trace_id, "yt_dlp_stdout", &trimmed);
                }
            }
            _ = &mut cancel_fut => {
                log_trace(trace_id, "download_cancelled", "cancel signal during download");
                let _ = child.kill().await;
                edit_status(
                    &api,
                    status_chat_id,
                    status_message_id,
                    t("youtube.download.cancelled"),
                )
                .await;
                cleanup_dir(&dir, trace_id).await;
                return;
            }
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

    let codec_name = t(selection.codec.label_key());
    let bitrate_str = find_format(&req, height, codec)
        .and_then(|f| f.bitrate)
        .map(|b| format!("{:.0}", b))
        .unwrap_or_else(|| "?".to_string());
    let thumb_path = fetch_thumbnail(&req.thumbnail_url, &dir, trace_id).await;

    // Check file size — split if > 2000 MB
    const MAX_SIZE_MB: u64 = 2000;
    const TARGET_PART_MB: u64 = 1700;
    let file_size_mb = tokio::fs::metadata(&path).await
        .map(|m| m.len() / (1024 * 1024))
        .unwrap_or(0);

    log_trace(
        trace_id,
        "upload_size_check",
        &format!("size_mb={file_size_mb} max_mb={MAX_SIZE_MB}"),
    );

    if file_size_mb > MAX_SIZE_MB {
        // Need to split
        let num_parts = ((file_size_mb + TARGET_PART_MB - 1) / TARGET_PART_MB) as usize;
        log_trace(trace_id, "split_needed", &format!("size_mb={file_size_mb} parts={num_parts}"));
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            tf("youtube.download.splitting", &[("parts", &num_parts.to_string())]),
        )
        .await;

        let part_paths = match split_video(&path, &dir, num_parts, req.duration, trace_id).await {
            Ok(parts) => parts,
            Err(e) => {
                log_trace(trace_id, "split_failed", &e);
                edit_status(
                    &api,
                    status_chat_id,
                    status_message_id,
                    tf("youtube.download.split_failed", &[("error", &e)]),
                )
                .await;
                cleanup_dir(&dir, trace_id).await;
                return;
            }
        };

        let total = part_paths.len();
        for (i, part_path) in part_paths.iter().enumerate() {
            let part_num = i + 1;

            // Verify part size
            let part_size_mb = tokio::fs::metadata(part_path).await
                .map(|m| m.len() / (1024 * 1024))
                .unwrap_or(0);
            log_trace(
                trace_id,
                "split_part_size",
                &format!("part={part_num}/{total} size_mb={part_size_mb} path={part_path}"),
            );
            if part_size_mb > MAX_SIZE_MB {
                log_trace(trace_id, "split_part_too_large", &format!("part={part_num} size_mb={part_size_mb}"));
                edit_status(
                    &api,
                    status_chat_id,
                    status_message_id,
                    tf("youtube.download.split_failed", &[("error", &format!("part {part_num} still {part_size_mb}MB after split"))]),
                )
                .await;
                cleanup_dir(&dir, trace_id).await;
                return;
            }

            edit_status(
                &api,
                status_chat_id,
                status_message_id,
                tf("youtube.download.uploading_part", &[
                    ("part", &part_num.to_string()),
                    ("total", &total.to_string()),
                ]),
            )
            .await;

            let caption = tf(
                "youtube.download.caption_part",
                &[
                    ("title", &req.title),
                    ("quality", &quality_label),
                    ("codec", &codec_name),
                    ("bitrate", &bitrate_str),
                    ("part", &part_num.to_string()),
                    ("total", &total.to_string()),
                ],
            );
            let caption_entities = entities_for_text(&caption);
            let mut params = SendVideoParams::builder()
                .chat_id(req.chat_id)
                .video(FileUpload::InputFile(InputFile { path: PathBuf::from(part_path) }))
                .supports_streaming(true)
                .caption(caption)
                .build();
            if !caption_entities.is_empty() {
                params.caption_entities = Some(caption_entities);
            }
            if part_num == 1 {
                if let Some(ref tp) = thumb_path {
                    params.thumbnail = Some(FileUpload::InputFile(InputFile { path: PathBuf::from(tp) }));
                }
            }
            params.height = Some(height);
            params.width = Some(height * 16 / 9);

            log_trace(trace_id, "upload_part_start", &format!("part={part_num}/{total} path={part_path}"));

            let api_for_send = api.clone();
            let mut send_task = tokio::spawn(async move { api_for_send.send_video(&params).await });
            let upload_start = Instant::now();
            let mut interval = tokio::time::interval(EDIT_THROTTLE);
            interval.tick().await;

            let send_result = loop {
                tokio::select! {
                    result = &mut send_task => { break result; }
                    _ = interval.tick() => {
                        let elapsed = upload_start.elapsed();
                        log_trace(trace_id, "upload_part_progress", &format!("part={part_num} elapsed={}s", elapsed.as_secs()));
                        edit_progress_status(
                            &api,
                            status_chat_id,
                            status_message_id,
                            format_upload_body(&quality_label, elapsed),
                            request_id,
                        )
                        .await;
                    }
                    _ = &mut cancel_fut => {
                        log_trace(trace_id, "upload_part_cancelled", &format!("part={part_num}"));
                        send_task.abort();
                        edit_status(&api, status_chat_id, status_message_id, t("youtube.download.cancelled")).await;
                        cleanup_dir(&dir, trace_id).await;
                        return;
                    }
                }
            };

            match send_result {
                Ok(Ok(_)) => {
                    log_trace(trace_id, "upload_part_ok", &format!("part={part_num}/{total} elapsed={}s", upload_start.elapsed().as_secs()));
                }
                Ok(Err(e)) => {
                    log_trace(trace_id, "upload_part_failed", &format!("part={part_num} err={e}"));
                    let _ = api.send_message(
                        &SendMessageParams::builder()
                            .chat_id(req.chat_id)
                            .text(tf("youtube.download.upload_part_failed", &[
                                ("part", &part_num.to_string()),
                                ("error", &e.to_string()),
                            ]))
                            .build(),
                    ).await;
                    cleanup_dir(&dir, trace_id).await;
                    return;
                }
                Err(e) => {
                    log_trace(trace_id, "upload_part_join_failed", &e.to_string());
                }
            }
        }

        // All parts uploaded
        let _ = api
            .delete_message(
                &frankenstein::methods::DeleteMessageParams::builder()
                    .chat_id(status_chat_id)
                    .message_id(status_message_id)
                    .build(),
            )
            .await;
    } else {
        // Normal single-file upload
        edit_status(
            &api,
            status_chat_id,
            status_message_id,
            t("youtube.download.uploading"),
        )
        .await;

        let caption = tf(
            "youtube.download.caption",
            &[
                ("title", &req.title),
                ("quality", &quality_label),
                ("codec", &codec_name),
                ("bitrate", &bitrate_str),
            ],
        );
        let caption_entities = entities_for_text(&caption);
        let mut params = SendVideoParams::builder()
            .chat_id(req.chat_id)
            .video(FileUpload::InputFile(InputFile {
                path: PathBuf::from(&path),
            }))
            .supports_streaming(true)
            .caption(caption)
            .build();
        if !caption_entities.is_empty() {
            params.caption_entities = Some(caption_entities);
        }
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
        params.width = Some(height * 16 / 9);

        log_trace(
            trace_id,
            "upload_start",
            &format!("path={path} thumb={}", thumb_path.as_deref().unwrap_or("none")),
        );

        let api_for_send = api.clone();
        let mut send_task = tokio::spawn(async move { api_for_send.send_video(&params).await });

        let upload_start = Instant::now();
        let mut interval = tokio::time::interval(EDIT_THROTTLE);
        interval.tick().await;

        let send_result = loop {
            tokio::select! {
                result = &mut send_task => { break result; }
                _ = interval.tick() => {
                    let elapsed = upload_start.elapsed();
                    log_trace(trace_id, "upload_progress", &format!("elapsed={}s", elapsed.as_secs()));
                    edit_progress_status(
                        &api,
                        status_chat_id,
                        status_message_id,
                        format_upload_body(&quality_label, elapsed),
                        request_id,
                    )
                    .await;
                }
                _ = &mut cancel_fut => {
                    log_trace(trace_id, "upload_cancelled", "cancel signal during upload");
                    send_task.abort();
                    edit_status(
                        &api,
                        status_chat_id,
                        status_message_id,
                        t("youtube.download.cancelled"),
                    )
                    .await;
                    cleanup_dir(&dir, trace_id).await;
                    return;
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
    }

    cleanup_dir(&dir, trace_id).await;
}

async fn split_video(
    input: &str,
    dir: &std::path::Path,
    num_parts: usize,
    duration_secs: Option<u64>,
    trace_id: u64,
) -> Result<Vec<String>, String> {
    // Get duration via ffprobe if not available from metadata
    let total_secs = match duration_secs.filter(|&d| d > 0) {
        Some(d) => d,
        None => {
            let out = tokio::process::Command::new("ffprobe")
                .args(["-v", "error", "-show_entries", "format=duration",
                       "-of", "default=noprint_wrappers=1:nokey=1", input])
                .output()
                .await
                .map_err(|e| format!("ffprobe spawn: {e}"))?;
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse::<f64>()
                .map(|f| f.round() as u64)
                .map_err(|_| "ffprobe: could not parse duration".to_string())?
        }
    };

    if total_secs == 0 {
        return Err("video duration is zero".to_string());
    }

    let part_secs = (total_secs + num_parts as u64 - 1) / num_parts as u64;
    log_trace(
        trace_id,
        "split_plan",
        &format!("total_secs={total_secs} parts={num_parts} part_secs={part_secs}"),
    );

    let mut parts = Vec::new();
    for i in 0..num_parts {
        let start = i as u64 * part_secs;
        if start >= total_secs {
            break;
        }
        let out_path = dir.join(format!("part{:02}.mp4", i + 1));
        let out_str = out_path.to_string_lossy().into_owned();

        log_trace(
            trace_id,
            "split_part_start",
            &format!("part={} start={start}s duration={part_secs}s out={out_str}", i + 1),
        );

        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-ss", &start.to_string(),
                "-i", input,
                "-t", &part_secs.to_string(),
                "-c", "copy",
                "-avoid_negative_ts", "make_zero",
                &out_str,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|e| format!("ffmpeg spawn part {}: {e}", i + 1))?;

        if !status.success() {
            return Err(format!("ffmpeg exit {} on part {}", status, i + 1));
        }

        log_trace(trace_id, "split_part_done", &format!("part={} path={out_str}", i + 1));
        parts.push(out_str);
    }

    Ok(parts)
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
