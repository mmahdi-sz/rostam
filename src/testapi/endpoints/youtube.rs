use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct YoutubeFormatReq {
    pub url: String,
}

#[derive(Serialize)]
pub struct YoutubeFormatResp {
    pub ok: bool,
    pub detected_url: String,
    pub title_sample: String,
}

pub async fn test_youtube_format(
    Json(req): Json<YoutubeFormatReq>,
) -> (axum::http::StatusCode, Json<YoutubeFormatResp>) {
    let is_yt = req.url.contains("youtube.com") || req.url.contains("youtu.be");
    (
        axum::http::StatusCode::OK,
        Json(YoutubeFormatResp {
            ok: is_yt,
            detected_url: req.url,
            title_sample: if is_yt {
                "Sample YouTube Title".into()
            } else {
                "Invalid URL".into()
            },
        }),
    )
}

#[derive(Deserialize)]
pub struct QualitySelectReq {
    pub request_id: u64,
    pub height: u32,
}

#[derive(Serialize)]
pub struct QualitySelectResp {
    pub ok: bool,
    pub request_id: u64,
    pub height: u32,
    pub callback_data: String,
}

pub async fn test_youtube_quality_select(
    Json(req): Json<QualitySelectReq>,
) -> Json<QualitySelectResp> {
    let cb = format!("yt:q:{}:{}", req.request_id, req.height);
    Json(QualitySelectResp {
        ok: true,
        request_id: req.request_id,
        height: req.height,
        callback_data: cb,
    })
}

#[derive(Deserialize)]
pub struct CancelReq {
    pub request_id: u64,
}

#[derive(Serialize)]
pub struct CancelResp {
    pub ok: bool,
    pub request_id: u64,
    pub cancelled: bool,
}

pub async fn test_youtube_cancel(Json(req): Json<CancelReq>) -> Json<CancelResp> {
    let cancelled = crate::youtube::download::cancel_download(req.request_id);
    Json(CancelResp {
        ok: true,
        request_id: req.request_id,
        cancelled,
    })
}

#[derive(Deserialize)]
pub struct SubtitleReq {
    pub scenario: String,
}

/// Exercises the real subtitle encoder and delivery probe using tiny local fixtures.
pub async fn test_youtube_subtitle(Json(req): Json<SubtitleReq>) -> Json<serde_json::Value> {
    let trace = crate::log::next_trace_id();
    crate::youtube::trace::log_trace(
        trace,
        "test_subtitle",
        &format!("scenario={} => start", req.scenario),
    );
    let result: anyhow::Result<serde_json::Value> = async {
        let dir = crate::common::dir::TempDirGuard::create("yt_subtitle_test", trace)?;
        let api = frankenstein::client_reqwest::Bot::new_url("http://127.0.0.1:14379/bot".to_string());
        let langs = vec!["fa".to_string()];
        if req.scenario == "rolling" {
            let raw = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/youtube_rolling.srt"));
            let cleaned = crate::youtube::download::subtitle::prepare_subtitle(raw, "en");
            let cues = crate::youtube::translator::parse_srt(&cleaned);
            anyhow::ensure!(cues.len() == 7 && cues[1].text == "next, cuz obviously the Democrats just", "rolling carryover remains");
            let rtl = crate::youtube::download::subtitle::prepare_subtitle("1\n00:00:00,000 --> 00:00:01,000\nYouTube نسخه 2026: سلام!\n\n", "fa");
            anyhow::ensure!(rtl.contains("\u{202b}YouTube نسخه 2026: سلام!\u{202c}"), "Persian direction missing");
            return Ok(serde_json::json!({"ok": true, "cues": cues.len(), "rtl": true, "trace": trace}));
        }
        if req.scenario == "missing" {
            let result = crate::youtube::download::subtitle::hardsub_subtitles(&api, 0, 0, dir.path(), "missing.mkv", &langs, Some(1), trace, 0).await;
            anyhow::ensure!(result.is_err(), "missing subtitle incorrectly succeeded");
            return Ok(serde_json::json!({"ok": true, "rejected": true, "trace": trace}));
        }
        if req.scenario == "translation" {
            tokio::fs::write(dir.path().join("sub.en-orig.srt"), include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/youtube_rolling.srt"))).await?;
            crate::youtube::download::subtitle::translate_subtitles(&api, 0, 0, trace, dir.path(), &langs, 0).await.map_err(anyhow::Error::msg)?;
            anyhow::ensure!(crate::youtube::download::subtitle::subtitle_file_exists_for_lang(dir.path(), "fa"), "local translation missing");
            crate::youtube::download::subtitle::clean_subtitles_in_dir(dir.path()).map_err(anyhow::Error::msg)?;
            let translated = tokio::fs::read_to_string(dir.path().join("translated_fa.srt")).await?;
            let cues = crate::youtube::translator::parse_srt(&translated);
            anyhow::ensure!(cues.len() == 7 && cues.iter().all(|c| c.text.starts_with('\u{202b}')), "translation did not receive cleaned cues or Persian direction");
        } else {
            anyhow::ensure!(req.scenario == "hardsub" || req.scenario == "remux" || req.scenario == "codec", "unknown scenario");
            tokio::fs::write(dir.path().join("sub.fa.srt"), "1\n00:00:00,000 --> 00:00:01,000\nسلام دنیا\n").await?;
        }
        let video = dir.path().join("input.mkv");
        let fixture_codec = if req.scenario == "remux" { "libx264" } else { "ffv1" };
        let fixture = tokio::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=black:s=320x180:d=1", "-c:v", fixture_codec, "-threads", "1"]).arg(&video).output().await?;
        anyhow::ensure!(fixture.status.success(), "fixture failed");
        let path = if req.scenario == "remux" || req.scenario == "codec" { video.to_string_lossy().into_owned() } else {
            crate::youtube::download::subtitle::hardsub_subtitles(&api, 0, 0, dir.path(), &video.to_string_lossy(), &langs, Some(1), trace, 0).await.map_err(anyhow::Error::msg)?
        };
        let (output_path, meta) = crate::youtube::download::helpers::prepare_video_output(&path, trace).await?;
        let is_video = meta.codec == "h264";
        if req.scenario == "codec" {
            anyhow::ensure!(!is_video && meta.codec == "ffv1", "codec incorrectly reported as H264");
        } else {
            anyhow::ensure!(is_video && output_path.ends_with(".mp4"), "output is not H264 MP4");
        }
        if req.scenario == "hardsub" || req.scenario == "translation" {
            let pixels = tokio::process::Command::new("ffmpeg").args(["-v", "error", "-i", &output_path, "-frames:v", "1", "-pix_fmt", "gray", "-f", "rawvideo", "pipe:1"]).output().await?;
            anyhow::ensure!(pixels.status.success() && pixels.stdout.iter().filter(|pixel| **pixel > 200).count() > 20, "subtitle pixels absent from black fixture");
        }
        Ok(serde_json::json!({"ok": true, "codec": meta.codec, "codec_label": crate::youtube::download::helpers::output_codec_label(&meta.codec), "container": if is_video { "mp4" } else { "mkv" }, "send_method": if is_video { "sendVideo" } else { "sendDocument" }, "trace": trace}))
    }.await;
    match result {
        Ok(result) => Json(result),
        Err(e) => Json(serde_json::json!({"ok": false, "error": e.to_string(), "trace": trace})),
    }
}
