use std::path::PathBuf;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::SendDocumentParams,
    types::InlineKeyboardMarkup,
};

use crate::i18n::tf;

use crate::emoji::panel::btn_icon_url;

use frankenstein::methods::SendMessageParams;
use redis::aio::MultiplexedConnection;
use tokio::sync::OnceCell;

static REDIS_CONN: OnceCell<MultiplexedConnection> = OnceCell::const_new();

async fn redis_conn() -> Option<MultiplexedConnection> {
    REDIS_CONN
        .get_or_try_init(|| async {
            let client = redis::Client::open(crate::config::redis_url())?;
            client.get_multiplexed_async_connection().await
        })
        .await
        .ok()
        .cloned()
}

const VLC_NOTICE_TTL_SECS: u64 = 48 * 3600; // 48 hours TTL

pub fn vlc_download_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                btn_icon_url(
                    "نسخه اندروید (Google Play)",
                    "https://play.google.com/store/apps/details?id=org.videolan.vlc",
                    "android_logo",
                ),
                btn_icon_url(
                    "نسخه iOS (App Store)",
                    "https://www.videolan.org/vlc/download-ios.html",
                    "app_store",
                ),
            ],
            vec![
                btn_icon_url(
                    "نسخه ویندوز",
                    "https://get.videolan.org/vlc/3.0.23/win32/vlc-3.0.23-win32.exe",
                    "windows_logo",
                ),
                btn_icon_url(
                    "نسخه لینوکس",
                    "https://www.videolan.org/vlc/#download",
                    "linux_logo",
                ),
            ],
        ])
        .build()
}

pub async fn maybe_send_non_h264_notice(
    api: &Bot,
    chat_id: i64,
    codec_name: &str,
    trace_id: u64,
) {
    let key = format!("user:notice:vlc:{chat_id}");

    if let Some(mut conn) = redis_conn().await {
        let exists: u32 = redis::cmd("EXISTS")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap_or(0);
        if exists > 0 {
            log_trace(trace_id, "vlc_notice_throttled", &format!("chat_id={chat_id} ttl=48h_active"));
            return;
        }

        let _: Result<(), _> = redis::cmd("SET")
            .arg(&key)
            .arg("1")
            .arg("EX")
            .arg(VLC_NOTICE_TTL_SECS)
            .query_async(&mut conn)
            .await;
    }

    let notice = tf("youtube.download.non_h264_notice", &[("codec", codec_name)]);
    let _ = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(notice)
                .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(
                    vlc_download_keyboard(),
                ))
                .build(),
        )
        .await;
    log_trace(trace_id, "vlc_notice_sent", &format!("chat_id={chat_id} ttl=48h"));
}

use super::super::trace::log_trace;

pub fn pick_largest_file(dir: &std::path::Path) -> Option<String> {
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

pub async fn cleanup_dir(dir: &std::path::Path, trace_id: u64) {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(_) => log_trace(trace_id, "cleanup_ok", &dir.display().to_string()),
        Err(e) => log_trace(trace_id, "cleanup_failed", &e.to_string()),
    }
}

pub async fn fetch_thumbnail(
    url: &Option<String>,
    dir: &std::path::Path,
    trace_id: u64,
) -> Option<String> {
    let url = url.as_deref()?;
    let raw_path = dir.join("thumb_raw");
    let jpg_path = dir.join("thumb.jpg");

    let resp = match reqwest::get(url).await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => { log_trace(trace_id, "thumb_http_error", &format!("status={}", r.status())); return None; }
        Err(e) => { log_trace(trace_id, "thumb_fetch_failed", &e.to_string()); return None; }
    };
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => { log_trace(trace_id, "thumb_bytes_failed", &e.to_string()); return None; }
    };
    if tokio::fs::write(&raw_path, &bytes).await.is_err() {
        log_trace(trace_id, "thumb_write_failed", url);
        return None;
    }
    log_trace(trace_id, "thumb_fetched", &format!("bytes={} raw={}", bytes.len(), raw_path.display()));

    // YouTube often returns WebP; convert to JPEG so Telegram accepts it as a thumbnail.
    let ffmpeg_out = tokio::process::Command::new("ffmpeg")
        .args(["-y", "-i", &raw_path.to_string_lossy(),
               "-vf", "scale=320:-1", "-q:v", "2",
               &jpg_path.to_string_lossy()])
        .output()
        .await;

    match ffmpeg_out {
        Ok(out) if out.status.success() => {
            log_trace(trace_id, "thumb_converted", &jpg_path.display().to_string());
            Some(jpg_path.to_string_lossy().into_owned())
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            log_trace(trace_id, "thumb_convert_failed", &format!("ffmpeg: {stderr}"));
            None
        }
        Err(e) => { log_trace(trace_id, "thumb_convert_spawn_failed", &e.to_string()); None }
    }
}

pub fn quality_label_for(height: u32) -> String {
    let key = format!("youtube.quality.buttons.{height}");
    let label = crate::i18n::t(&key);
    if label.starts_with('!') {
        format!("{height}p")
    } else {
        label
    }
}

/// Finds subtitle files (.srt/.vtt) produced by yt-dlp in `dir` and sends each
/// to the user as a document. Used in SubtitleMode::File. Returns how many were sent.
pub async fn send_subtitle_files(
    api: &Bot,
    dir: &std::path::Path,
    chat_id: i64,
    video_title: &str,
    trace_id: u64,
) -> usize {
    let mut sent = 0usize;
    let Ok(entries) = std::fs::read_dir(dir) else { return 0; };
    let mut subs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let e = e.to_ascii_lowercase();
                    e == "srt" || e == "vtt"
                })
                .unwrap_or(false)
        })
        .collect();
    subs.sort();
    for sub_path in &subs {
        let fname = sub_path.file_name().and_then(|n| n.to_str()).unwrap_or("subtitle");
        // Try to surface the language tag in the caption (e.g. "video.fa.srt" -> "fa").
        let lang = sub_path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.rsplit('.').next())
            .unwrap_or("")
            .to_string();
        let caption = tf("youtube.download.subtitle_caption", &[
            ("title", video_title), ("lang", &lang),
        ]);
        let params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(FileUpload::InputFile(InputFile { path: sub_path.clone() }))
            .caption(caption)
            .build();
        match api.send_document(&params).await {
            Ok(_) => {
                sent += 1;
                log_trace(trace_id, "subtitle_file_sent", &format!("file={fname} lang={lang}"));
            }
            Err(e) => log_trace(trace_id, "subtitle_file_failed", &format!("file={fname} err={e}")),
        }
    }
    log_trace(trace_id, "subtitle_files_done", &format!("sent={sent} found={}", subs.len()));
    sent
}

pub async fn translate_subtitles(
    api: &Bot,
    chat_id: i64,
    msg_id: i32,
    trace_id: u64,
    dir: &std::path::Path,
    target_langs: &[String],
) -> Result<(), String> {
    if target_langs.is_empty() {
        return Ok(());
    }

    let mut has_target = false;
    let mut english_srt = None;
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()); };
    let mut srts = Vec::new();
    
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("srt") {
            let fname = path.file_name().unwrap_or_default().to_str().unwrap_or_default().to_string();
            srts.push(path.clone());
            for tgt in target_langs {
                if fname.contains(&format!(".{}.", tgt)) || fname.contains(&format!(".{}-", tgt)) || fname.contains(&format!("_{}.srt", tgt)) {
                    has_target = true;
                }
            }
            if fname.contains(".en.") || fname.contains(".en-") || fname.contains("_en.srt") {
                english_srt = Some(path.clone());
            }
        }
    }
    
    if has_target || english_srt.is_none() {
        return Ok(());
    }
    
    let english_srt = english_srt.unwrap();
    
    if msg_id > 0 {
        crate::youtube::download::status::edit_status(api, chat_id, msg_id, crate::i18n::tf("youtube.download.translating_subtitle", &[])).await;
    }
    
    for tgt in target_langs {
        if tgt == "en" || tgt.starts_with("en-") { continue; }

        let Some(nllb_lang) = crate::youtube::translator::map_language_code(tgt) else { continue; };

        let out_path = dir.join(format!("translated_{}.srt", tgt));

        let cores = acquire_cpu(0, trace_id).await;
        let num_threads = if cores.is_empty() { 4 } else { cores.len() };

        let res = crate::youtube::translator::translate_srt(
            &english_srt,
            &out_path,
            nllb_lang,
            num_threads,
        ).await;

        release_cpu(cores, trace_id).await;

        match res {
            Ok(()) => {
                crate::youtube::trace::log_trace(trace_id, "translate_subtitle_ok", &format!("lang={tgt}"));
            }
            Err(e) => {
                crate::youtube::trace::log_trace(trace_id, "translate_subtitle_error", &format!("lang={tgt} err={e}"));
            }
        }
    }
    Ok(())
}

/// تضمین می‌کند که برای زبان‌های مقصدِ خواسته‌شده زیرنویس وجود دارد — مستقل از
/// حالت تحویل (File یا Embedded). اگر کاربر زبانی (مثل fa) خواسته که یوتیوب
/// نداشته، انگلیسی را جدا دانلود و به‌صورت محلی (NLLB) ترجمه می‌کند. خروجی
/// فایل‌های `translated_<lang>.srt` در همان پوشه است. اگر زیرنویس مقصد از قبل
/// موجود باشد یا زبان غیرقابل‌ترجمه باشد، عملاً no-op است.
///
/// این تابع «مرحله‌ی ترجمه» است؛ تقسیم به File/Embedded بعد از آن انجام می‌شود.
pub async fn ensure_translated_subtitles(
    api: &Bot,
    cookie_spec: &str,
    webpage_url: &str,
    chat_id: i64,
    msg_id: i32,
    dir: &std::path::Path,
    target_langs: &[String],
    trace_id: u64,
) {
    // فقط وقتی کاری داریم که زبان غیرانگلیسیِ قابل‌ترجمه‌ای خواسته شده باشد.
    let needs = target_langs
        .iter()
        .any(|l| l != "en" && !l.starts_with("en-") && crate::youtube::translator::map_language_code(l).is_some());
    if !needs {
        return;
    }

    // انگلیسی را (در کنار زبان‌های خواسته‌شده) جدا می‌گیریم تا مبنای ترجمه باشد.
    download_subtitles_separately(cookie_spec, webpage_url, dir, target_langs, trace_id).await;

    if let Err(e) = translate_subtitles(api, chat_id, msg_id, trace_id, dir, target_langs).await {
        crate::youtube::trace::log_trace(trace_id, "ensure_translate_failed", &format!("err={e}"));
    }
}

pub async fn embed_subtitles(
    dir: &std::path::Path,
    video_path: &str,
    trace_id: u64,
) -> Result<String, String> {
    crate::youtube::trace::log_trace(trace_id, "embed_subtitles_started", "");
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(video_path.to_string()); };
    let mut srts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("srt") {
            srts.push(path);
        }
    }
    
    if srts.is_empty() {
        return Ok(video_path.to_string());
    }
    
    srts.sort_by(|a, b| {
        let a_name = a.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        let b_name = b.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        let a_is_fa = a_name.contains("translated_fa") || a_name.contains(".fa.") || a_name.contains("_fa.srt");
        let b_is_fa = b_name.contains("translated_fa") || b_name.contains(".fa.") || b_name.contains("_fa.srt");
        match (a_is_fa, b_is_fa) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a_name.cmp(&b_name),
        }
    });
    
    let out_path = format!("{}_embedded.mp4", video_path.trim_end_matches(".mp4"));
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.arg("-y").arg("-i").arg(video_path);
    
    for srt in &srts {
        cmd.arg("-i").arg(srt.to_str().unwrap());
    }
    
    cmd.arg("-c").arg("copy");
    cmd.arg("-c:s").arg("mov_text");
    
    cmd.arg("-map").arg("0");
    for (i, srt) in srts.iter().enumerate() {
        let idx = i + 1;
        cmd.arg("-map").arg(idx.to_string());
        
        let fname = srt.file_name().unwrap_or_default().to_str().unwrap_or_default().to_lowercase();
        let (lang_code, lang_title) = if fname.contains("translated_fa") || fname.contains(".fa.") || fname.contains("_fa.srt") {
            ("per", "زیرنویس فارسی (Farsi)")
        } else if fname.contains(".en.") || fname.contains("_en.srt") || fname.contains("translated_en") {
            ("eng", "English Subtitle")
        } else {
            ("und", "Subtitle")
        };
        
        cmd.arg(format!("-metadata:s:s:{}", i))
           .arg(format!("language={}", lang_code));
        cmd.arg(format!("-metadata:s:s:{}", i))
           .arg(format!("title={}", lang_title));
        cmd.arg(format!("-metadata:s:s:{}", i))
           .arg(format!("handler_name={}", lang_title));

        if i == 0 {
            cmd.arg("-disposition:s:0").arg("default");
        } else {
            cmd.arg(format!("-disposition:s:{}", i)).arg("0");
        }
    }
    cmd.arg(&out_path);
    
    let out = cmd.output().await.map_err(|e| e.to_string())?;
    if out.status.success() {
        let _ = tokio::fs::remove_file(video_path).await;
        for srt in &srts {
            let _ = tokio::fs::remove_file(srt).await;
        }
        Ok(out_path)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

/// ffmpeg (and therefore yt-dlp --embed-subs) writes mov_text/tx3g sample
/// entries with displayFlags=0. Players such as VLC then list the subtitle
/// track in the menu but never render it unless the user selects it manually
/// (the auto-selected track is decoded yet not displayed). Setting the tx3g
/// "forced" display flags (0xC0000000) on the first enabled subtitle track
/// makes players auto-display it, which is what a soft-sub user expects.
/// Patches 4 bytes in place; never touches sample data.
pub async fn fix_embedded_subtitle_flags(video_path: &str, trace_id: u64) -> bool {
    log_trace(trace_id, "subtitle_flags_fix", &format!("path={video_path}"));
    let path = video_path.to_string();
    let res = tokio::task::spawn_blocking(move || patch_first_tx3g_display_flags(&path)).await;
    match res {
        Ok(Ok(Some(offset))) => {
            log_trace(trace_id, "subtitle_flags_fix", &format!("=> ok offset={offset}"));
            true
        }
        Ok(Ok(None)) => {
            log_trace(trace_id, "subtitle_flags_fix", "=> pass no enabled tx3g track");
            false
        }
        Ok(Err(e)) => {
            log_trace(trace_id, "subtitle_flags_fix", &format!("=> fail err={e}"));
            false
        }
        Err(e) => {
            log_trace(trace_id, "subtitle_flags_fix", &format!("=> fail join err={e}"));
            false
        }
    }
}

/// tx3g displayFlags bits 0x80000000|0x40000000 = "all/some samples forced".
const TX3G_FORCED_FLAGS: [u8; 4] = [0xC0, 0x00, 0x00, 0x00];

/// Finds the first child box named `name` in `buf[off..end]`.
fn find_box(buf: &[u8], mut off: usize, end: usize, name: &[u8; 4]) -> Option<(usize, usize)> {
    while off + 8 <= end.min(buf.len()) {
        let size = u32::from_be_bytes(buf[off..off + 4].try_into().unwrap()) as usize;
        if size < 8 || off + size > end { return None; }
        if &buf[off + 4..off + 8] == name {
            return Some((off, size));
        }
        off += size;
    }
    None
}

/// Locates the first *enabled* trak whose stsd sample entry is tx3g and
/// overwrites its displayFlags with the forced flags. Returns the absolute
/// file offset patched, or None when the file has no such track.
fn patch_first_tx3g_display_flags(path: &str) -> std::io::Result<Option<u64>> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let file_len = f.metadata()?.len();

    // Top-level scan for the moov box (may sit before or after mdat).
    let mut off: u64 = 0;
    let mut moov: Option<(u64, u64)> = None;
    let mut hdr = [0u8; 8];
    while off + 8 <= file_len {
        f.seek(SeekFrom::Start(off))?;
        f.read_exact(&mut hdr)?;
        let mut size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        if size == 1 {
            let mut large = [0u8; 8];
            f.read_exact(&mut large)?;
            size = u64::from_be_bytes(large);
        } else if size == 0 {
            size = file_len - off;
        }
        if size < 8 { break; }
        if &hdr[4..8] == b"moov" {
            moov = Some((off, size));
            break;
        }
        off += size;
    }
    let Some((moov_off, moov_size)) = moov else { return Ok(None) };
    // moov of a 2GB video is a few MB; anything huge means a corrupt size field.
    if moov_size > 128 * 1024 * 1024 { return Ok(None); }

    let mut buf = vec![0u8; moov_size as usize];
    f.seek(SeekFrom::Start(moov_off))?;
    f.read_exact(&mut buf)?;

    let moov_end = buf.len();
    let mut t_off = 8usize;
    while let Some((trak, trak_size)) = find_box(&buf, t_off, moov_end, b"trak") {
        t_off = trak + trak_size;
        let trak_end = trak + trak_size;

        // tkhd flags bit 0 = track_enabled; a disabled track is never
        // auto-selected, so forcing its display would do nothing useful.
        let enabled = find_box(&buf, trak + 8, trak_end, b"tkhd")
            .map(|(o, s)| s >= 12 && (buf[o + 11] & 0x01) != 0)
            .unwrap_or(false);
        if !enabled { continue; }

        let Some((mdia, mdia_size)) = find_box(&buf, trak + 8, trak_end, b"mdia") else { continue };
        let Some((minf, minf_size)) = find_box(&buf, mdia + 8, mdia + mdia_size, b"minf") else { continue };
        let Some((stbl, stbl_size)) = find_box(&buf, minf + 8, minf + minf_size, b"stbl") else { continue };
        let Some((stsd, stsd_size)) = find_box(&buf, stbl + 8, stbl + stbl_size, b"stsd") else { continue };

        // stsd: 8 header + 4 version/flags + 4 entry_count, then first entry.
        let entry = stsd + 16;
        if entry + 20 > stsd + stsd_size { continue; }
        if &buf[entry + 4..entry + 8] != b"tx3g" { continue; }

        // tx3g entry: 4 size + 4 type + 6 reserved + 2 data_ref_index, then displayFlags.
        let abs = moov_off + (entry + 16) as u64;
        f.seek(SeekFrom::Start(abs))?;
        f.write_all(&TX3G_FORCED_FLAGS)?;
        return Ok(Some(abs));
    }
    Ok(None)
}

pub async fn download_subtitles_separately(
    cookie_spec: &str,
    webpage_url: &str,
    dir: &std::path::Path,
    target_langs: &[String],
    trace_id: u64,
) {
    if target_langs.is_empty() {
        return;
    }
    let mut target_langs = target_langs.to_vec();
    if !target_langs.contains(&"en".to_string()) {
        target_langs.push("en".to_string());
    }
    let sub_langs = target_langs.join(",");

    let mut cmd = tokio::process::Command::new("yt-dlp");
    cmd.arg("--js-runtimes").arg(format!("deno:{}", crate::config::deno_path()))
        .arg("--cookies-from-browser").arg(cookie_spec)
        .arg("--no-warnings").arg("--no-playlist")
        .arg("--write-subs").arg("--write-auto-subs")
        .arg("--sub-langs").arg(&sub_langs)
        .arg("--convert-subs").arg("srt")
        .arg("--skip-download")
        .arg("-o").arg(format!("{}/sub.%(ext)s", dir.display()))
        .arg(webpage_url);

    crate::youtube::trace::log_trace(trace_id, "download_subtitles_separately_start", &format!("langs={sub_langs}"));
    let _ = cmd.output().await;
}

const SEP_BASE: &str = "http://127.0.0.1:6589";

async fn acquire_cpu(user_id: i64, trace_id: u64) -> Vec<i32> {
    let client = reqwest::Client::new();
    let res = client
        .post(format!("{SEP_BASE}/cpu/acquire"))
        .form(&[("user_id", user_id.to_string()), ("is_vip", "false".to_string())])
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await;
    match res {
        Ok(r) => {
            let json: serde_json::Value = r.json().await.unwrap_or_default();
            let cores: Vec<i32> = json
                .get("cores")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            log_trace(trace_id, "cpu_acquired", &format!("{cores:?}"));
            cores
        }
        Err(e) => {
            log_trace(trace_id, "cpu_acquire_failed", &format!("{e}"));
            vec![]
        }
    }
}

async fn release_cpu(cores: Vec<i32>, trace_id: u64) {
    if cores.is_empty() { return; }
    let client = reqwest::Client::new();
    let body = serde_json::json!({ "cores": cores });
    let r = client
        .post(format!("{SEP_BASE}/cpu/release"))
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;
    log_trace(trace_id, "cpu_released", &format!("cores={cores:?} ok={}", r.is_ok()));
}


#[cfg(test)]
mod tests {
    use super::patch_first_tx3g_display_flags;

    // Builds a real mov_text mp4 with ffmpeg, patches it, and verifies via a
    // fresh parse that only displayFlags changed and it now reads 0xC0000000.
    #[test]
    fn tx3g_display_flags_patch_roundtrip() {
        let dir = std::env::temp_dir().join(format!("tx3g_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let srt = dir.join("s.srt");
        std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:02,000\nhello\n\n").unwrap();
        let out = dir.join("o.mp4");
        let st = std::process::Command::new("ffmpeg")
            .args(["-y", "-v", "error",
                   "-f", "lavfi", "-i", "color=black:size=64x64:rate=10:duration=3",
                   "-i", srt.to_str().unwrap(),
                   "-c:v", "libx264", "-preset", "ultrafast",
                   "-c:s", "mov_text",
                   "-map", "0", "-map", "1",
                   out.to_str().unwrap()])
            .status()
            .expect("ffmpeg must exist on host");
        assert!(st.success());

        let before = std::fs::read(&out).unwrap();
        let offset = patch_first_tx3g_display_flags(out.to_str().unwrap())
            .unwrap()
            .expect("must find a tx3g track") as usize;
        let after = std::fs::read(&out).unwrap();

        assert_eq!(before.len(), after.len(), "patch must not change file size");
        // The patched offset must be displayFlags right inside a tx3g entry:
        // entry starts 16 bytes earlier, fourcc at entry+4.
        assert_eq!(&after[offset - 12..offset - 8], b"tx3g");
        assert_eq!(&after[offset..offset + 4], &[0xC0, 0x00, 0x00, 0x00]);
        // Everything else byte-identical.
        assert!(before[..offset] == after[..offset]);
        assert!(before[offset + 4..] == after[offset + 4..]);

        std::fs::remove_dir_all(&dir).ok();
    }


    #[test]
    fn tx3g_patch_ignores_files_without_subtitles() {
        let dir = std::env::temp_dir().join(format!("tx3g_nosub_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("o.mp4");
        let st = std::process::Command::new("ffmpeg")
            .args(["-y", "-v", "error",
                   "-f", "lavfi", "-i", "color=black:size=64x64:rate=10:duration=1",
                   "-c:v", "libx264", "-preset", "ultrafast",
                   out.to_str().unwrap()])
            .status()
            .expect("ffmpeg must exist on host");
        assert!(st.success());

        let before = std::fs::read(&out).unwrap();
        let res = patch_first_tx3g_display_flags(out.to_str().unwrap()).unwrap();
        assert!(res.is_none());
        assert_eq!(before, std::fs::read(&out).unwrap(), "file must be untouched");

        std::fs::remove_dir_all(&dir).ok();
    }
}

/// Create a clean, safe OS filename for downloaded video files.
/// Ensures technical info (e.g. [1080p AV1 1498kbps]) is strictly preserved,
/// while the title is sanitized and truncated if the total byte length exceeds 245 bytes.
pub fn sanitize_video_filename(
    title: &str,
    quality_label: &str,
    codec_name: &str,
    bitrate_str: &str,
    ext: &str,
) -> String {
    let tech_info = if bitrate_str != "?" && !bitrate_str.is_empty() {
        format!("[{quality_label} {codec_name} {bitrate_str}kbps]")
    } else {
        format!("[{quality_label} {codec_name}]")
    };
    let ext_with_dot = format!(".{ext}");
    let suffix = format!(" {tech_info}{ext_with_dot}");

    const MAX_TOTAL_BYTES: usize = 245;
    let suffix_bytes = suffix.len();
    let max_title_bytes = if MAX_TOTAL_BYTES > suffix_bytes {
        MAX_TOTAL_BYTES - suffix_bytes
    } else {
        50
    };

    let mut clean_title = String::with_capacity(title.len());
    for c in title.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => {
                clean_title.push(' ');
            }
            _ => clean_title.push(c),
        }
    }

    let mut normalized_title = clean_title.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized_title.is_empty() {
        normalized_title = "video".to_string();
    }

    if normalized_title.len() > max_title_bytes {
        let mut byte_count = 0;
        let mut cutoff = normalized_title.len();

        for (idx, ch) in normalized_title.char_indices() {
            let ch_len = ch.len_utf8();
            if byte_count + ch_len > max_title_bytes {
                cutoff = idx;
                break;
            }
            byte_count += ch_len;
        }

        normalized_title.truncate(cutoff);
        normalized_title = normalized_title.trim_end().to_string();
    }

    format!("{normalized_title}{suffix}")
}

/// Create a clean, safe OS filename for downloaded audio files.
pub fn sanitize_audio_filename(
    title: &str,
    quality_label: &str,
    ext: &str,
) -> String {
    let tech_info = format!("[{quality_label}]");
    let ext_with_dot = format!(".{ext}");
    let suffix = format!(" {tech_info}{ext_with_dot}");

    const MAX_TOTAL_BYTES: usize = 245;
    let suffix_bytes = suffix.len();
    let max_title_bytes = if MAX_TOTAL_BYTES > suffix_bytes {
        MAX_TOTAL_BYTES - suffix_bytes
    } else {
        50
    };

    let mut clean_title = String::with_capacity(title.len());
    for c in title.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => {
                clean_title.push(' ');
            }
            _ => clean_title.push(c),
        }
    }

    let mut normalized_title = clean_title.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized_title.is_empty() {
        normalized_title = "audio".to_string();
    }

    if normalized_title.len() > max_title_bytes {
        let mut byte_count = 0;
        let mut cutoff = normalized_title.len();

        for (idx, ch) in normalized_title.char_indices() {
            let ch_len = ch.len_utf8();
            if byte_count + ch_len > max_title_bytes {
                cutoff = idx;
                break;
            }
            byte_count += ch_len;
        }

        normalized_title.truncate(cutoff);
        normalized_title = normalized_title.trim_end().to_string();
    }

    format!("{normalized_title}{suffix}")
}
