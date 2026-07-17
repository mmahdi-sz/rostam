use std::path::PathBuf;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::SendDocumentParams,
};

use crate::i18n::tf;

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
        
        let nllb_lang = match tgt.as_str() {
            "fa" => "pes_Arab",
            "en" => "eng_Latn",
            "it" => "ita_Latn",
            "fr" => "fra_Latn",
            "de" => "deu_Latn",
            "ru" => "rus_Cyrl",
            "es" => "spa_Latn",
            "ar" => "arb_Arab",
            "hi" => "hin_Deva",
            "tr" => "tur_Latn",
            _ => continue,
        };
        
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

