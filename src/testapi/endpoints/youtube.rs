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
            let stacked = crate::youtube::download::subtitle::stack_subtitles_chronologically("1\n00:00:00,000 --> 00:00:04,000\nOld\n\n2\n00:00:02,000 --> 00:00:05,000\nNew\n\n");
            let blocks = crate::youtube::translator::parse_srt(&stacked);
            anyhow::ensure!(blocks.len() == 3 && blocks[1].text == "Old\nNew", "caption stacking order incorrect");

            let raw = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/youtube_rolling.srt"));
            let cleaned = crate::youtube::download::subtitle::prepare_subtitle(raw, "en");
            let cues = crate::youtube::translator::parse_srt(&cleaned);
            anyhow::ensure!(cues.len() == 7 && cues[1].text == "next, cuz obviously the Democrats just", "rolling carryover remains");
            let rtl = crate::youtube::download::subtitle::prepare_subtitle("1\n00:00:00,000 --> 00:00:01,000\nYouTube نسخه 2026: سلام!\n\n", "fa");
            anyhow::ensure!(rtl.contains("\u{202b}YouTube نسخه 2026: سلام!\u{202c}"), "Persian direction missing");
            return Ok(serde_json::json!({"ok": true, "cues": cues.len(), "rtl": true, "trace": trace}));
        }
        if req.scenario == "missing" {
            let result = crate::youtube::download::subtitle::hardsub_subtitles(&api, 0, 0, dir.path(), "missing.mkv", &langs, "h264", Some(1), trace, 0).await;
            anyhow::ensure!(result.is_err(), "missing subtitle incorrectly succeeded");
            return Ok(serde_json::json!({"ok": true, "rejected": true, "trace": trace}));
        }
        if matches!(req.scenario.as_str(), "translation_ja_en" | "translation_no_source") {
            let targets = vec!["en".to_string()];
            let timestamp = "00:00:01,000 --> 00:00:03,000";
            let has_source = req.scenario == "translation_ja_en";
            if has_source {
                tokio::fs::write(dir.path().join("sub.ja-orig.srt"), format!("1\n{timestamp}\n今日はいい天気です。\n")).await?;
            }
            let result = crate::youtube::download::subtitle::translate_subtitles(
                &api, 0, 0, trace, dir.path(), &targets, None, 0,
            ).await;
            if !has_source {
                anyhow::ensure!(result.as_ref().is_err_and(|e| e == "no supported source subtitle"), "missing source did not fail");
                return Ok(serde_json::json!({"ok": true, "rejected": true, "trace": trace}));
            }
            result.map_err(anyhow::Error::msg)?;
            let translated = tokio::fs::read_to_string(dir.path().join("translated_en.srt")).await?;
            let cues = crate::youtube::translator::parse_srt(&translated);
            anyhow::ensure!(cues.len() == 1 && cues[0].timestamp == timestamp && cues[0].text.to_lowercase().contains("weather"), "expected English translation with original timing");
            return Ok(serde_json::json!({"ok": true, "subtitle": translated, "trace": trace}));
        }
        if req.scenario == "translation" {
            tokio::fs::write(dir.path().join("sub.en-orig.srt"), include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/youtube_rolling.srt"))).await?;
            crate::youtube::download::subtitle::translate_subtitles(&api, 0, 0, trace, dir.path(), &langs, None, 0).await.map_err(anyhow::Error::msg)?;
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
            crate::youtube::download::subtitle::hardsub_subtitles(&api, 0, 0, dir.path(), &video.to_string_lossy(), &langs, "h264", Some(1), trace, 0).await.map_err(anyhow::Error::msg)?
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

pub async fn test_youtube_audio(Json(req): Json<serde_json::Value>) -> Json<serde_json::Value> {
    let trace = crate::log::next_trace_id();
    crate::log_actor_id!("youtube", trace, 12345, "clicked" => "test_audio");
    let result = async {
        use crate::youtube::download::{audio, status, types::AudioQuality, upload};
        use tokio::process::Command;

        let directory = crate::common::dir::TempDirGuard::create("test_youtube_audio", trace)?;
        let dir = directory.path();
        let scenario = req["scenario"].as_str().unwrap_or_default();
        if scenario == "oversized" {
            anyhow::ensure!(
                audio::size_error(2000 * 1024 * 1024).is_none(),
                "rejected allowed size"
            );
            let reason = audio::size_error(2000 * 1024 * 1024 + 1)
                .ok_or_else(|| anyhow::anyhow!("accepted oversized audio"))?;
            return Ok(
                serde_json::json!({"ok": true, "rejected": true, "caption": reason,
                "i18n_keys": ["youtube.audio.oversized"], "trace": trace}),
            );
        }
        if scenario == "invalid" {
            let error = audio::probe(dir.join("missing.m4a").to_str().unwrap_or_default()).await;
            anyhow::ensure!(error.is_err(), "accepted missing audio");
            return Ok(serde_json::json!({"ok": true, "rejected": true, "trace": trace}));
        }
        anyhow::ensure!(
            matches!(scenario, "m4a" | "mp3" | "single" | "upload_failed"),
            "unknown scenario"
        );
        let index = match scenario {
            "m4a" => Some(1),
            "single" => None,
            _ => Some(3),
        };
        let expected_track = index.unwrap_or(8).to_string();
        let extension = if scenario == "mp3" { "mp3" } else { "m4a" };
        let mut formats = Vec::new();
        for (ext, encoder, codec) in [("m4a", "aac", "aac"), ("webm", "libopus", "opus")] {
            if scenario == "mp3" && ext == "m4a" {
                continue;
            }
            let source = dir.join(format!("source.{ext}"));
            let output = Command::new(crate::config::ffmpeg_path())
                .args([
                    "-v", "error", "-f", "lavfi", "-i", "anullsrc", "-t", "0.2", "-c:a", encoder,
                ])
                .arg(&source)
                .output()
                .await?;
            anyhow::ensure!(
                output.status.success(),
                "fixture generation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            formats.push(serde_json::json!({
                "format_id": ext, "url": format!("file://{}", source.display()),
                "ext": ext, "vcodec": "none", "acodec": codec,
            }));
        }
        let info_path = dir.join("info.json");
        tokio::fs::write(
            &info_path,
            serde_json::to_vec(&serde_json::json!({
                "id": "fixture", "title": "Audio fixture", "uploader": "Fixture Artist",
                "album_artist": "Other Artist", "genre": "People & Blogs",
                "categories": ["People & Blogs"], "track_number": 8, "formats": formats,
            }))?,
        )
        .await?;
        let mut command = Command::new("yt-dlp");
        command
            .args(["--ignore-config", "--enable-file-urls", "--load-info-json"])
            .arg(info_path)
            .args(["-f", AudioQuality::Best.format_spec(), "-o"])
            .arg(dir.join("result.%(ext)s"));
        audio::configure(&mut command, index);
        let output = command.output().await?;
        anyhow::ensure!(
            output.status.success(),
            "yt-dlp failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let path = dir.join(format!("result.{extension}"));
        let meta = audio::probe(path.to_str().unwrap_or_default()).await?;
        let probe = Command::new(crate::config::ffprobe_path())
            .args(["-v", "error", "-show_entries", "format_tags", "-of", "json"])
            .arg(&path)
            .output()
            .await?;
        anyhow::ensure!(probe.status.success(), "ffprobe failed");
        let tags = &serde_json::from_slice::<serde_json::Value>(&probe.stdout)?["format"]["tags"];
        anyhow::ensure!(
            tags["artist"] == "Fixture Artist"
                && tags["album_artist"] == "Fixture Artist"
                && tags["genre"].is_null()
                && tags["track"] == expected_track,
            "unexpected tags: {tags}"
        );
        let caption = audio::caption(
            "Audio fixture",
            "Best",
            &meta.codec,
            "0",
            "https://youtu.be/fixture",
        );
        let entities = crate::i18n::entities_for_text(&caption);
        let port = std::env::var("TESTAPI_PORT").unwrap_or_else(|_| "14379".to_string());
        let api_base = if scenario == "upload_failed" {
            "missing"
        } else {
            "botdummy_token"
        };
        let api = frankenstein::client_reqwest::Bot::new_url(format!(
            "http://127.0.0.1:{port}/{api_base}"
        ));
        let mut pending = std::pin::pin!(std::future::pending());
        let sent = upload::send_audio_file(
            &api,
            12345,
            path.to_str().unwrap_or_default(),
            "Audio fixture".to_owned(),
            meta.artist.unwrap_or_default(),
            caption.clone(),
            entities.clone(),
            12345,
            999,
            trace,
            &mut pending,
            trace,
        )
        .await;
        anyhow::ensure!(
            sent == (scenario != "upload_failed"),
            "unexpected upload result"
        );
        Ok(serde_json::json!({
            "ok": true, "trace": trace, "codec": meta.codec, "extension": meta.ext,
            "tags": tags, "caption": caption, "i18n_keys": ["youtube.audio.caption"],
            "caption_entities": entities, "keyboard": status::cancel_keyboard(trace),
            "send_method": "sendAudio", "uploaded": sent, "actor_logged": true,
        }))
    }
    .await;
    Json(result.unwrap_or_else(|err: anyhow::Error| {
        serde_json::json!({
            "ok": false, "error": err.to_string(), "trace": trace,
        })
    }))
}
