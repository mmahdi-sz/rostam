use std::path::Path;

use crate::studio::trim::probe::{AudioMetadata, probe_audio_metadata};

pub(crate) fn configure(cmd: &mut tokio::process::Command, index: Option<u64>) {
    cmd.args([
        "--extract-audio",
        "--audio-format",
        "m4a>m4a/mp3",
        "--audio-quality",
        "0",
        "--embed-metadata",
        "--no-embed-chapters",
        "--embed-thumbnail",
        "--parse-metadata",
        "%(uploader,channel|)s:%(meta_artist)s",
        "--parse-metadata",
        "%(uploader,channel|)s:%(meta_album_artist)s",
        "--parse-metadata",
        "%(release_date,upload_date)s:%(meta_date)s",
        "--parse-metadata",
        "%(genre|)s:%(meta_genre)s",
        "--replace-in-metadata",
        "meta_genre",
        "(?i)^people & blogs?$",
        "",
    ]);
    if let Some(index) = index {
        cmd.arg("--parse-metadata")
            .arg(format!("{index}:%(meta_track)s"));
    }
}

pub(crate) async fn probe(path: &str) -> anyhow::Result<AudioMetadata> {
    let meta = probe_audio_metadata(Path::new(path)).await?;
    anyhow::ensure!(
        matches!(
            (meta.ext.as_str(), meta.codec.as_str()),
            ("m4a", "aac") | ("mp3", "mp3")
        ),
        "unexpected audio output: {}/{}",
        meta.ext,
        meta.codec
    );
    Ok(meta)
}

pub(crate) fn caption(title: &str, quality: &str, codec: &str, bitrate: &str, url: &str) -> String {
    crate::i18n::tf(
        "youtube.audio.caption",
        &[
            ("title", title),
            ("quality", quality),
            ("codec", &codec.to_ascii_uppercase()),
            ("bitrate", bitrate),
            ("username", crate::config::bot_username()),
            ("url", url),
        ],
    )
}

pub(super) fn find_output(dir: &Path) -> Option<String> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("m4a" | "mp3")
                )
        })
        .map(|path| path.to_string_lossy().into_owned())
}

pub(super) async fn broker_command(
    user_id: i64,
    trace: u64,
) -> Result<
    (
        crate::common::cpu_broker::CpuBrokerGuard,
        tokio::process::Command,
    ),
    String,
> {
    use crate::common::cpu_broker::CpuBrokerGuard;

    if CpuBrokerGuard::is_user_busy(user_id).await {
        crate::youtube::trace::log_trace(trace, "audio_cpu_busy", "blocked");
        return Err(crate::i18n::t("active_job_running"));
    }
    crate::youtube::trace::log_trace(trace, "audio_cpu_acquire", "waiting");
    let guard = CpuBrokerGuard::acquire(user_id, trace, "youtube").await;
    if !guard.is_acquired() {
        crate::youtube::trace::log_trace(trace, "audio_cpu_acquire_failed", "no allocation");
        return Err(crate::i18n::t("youtube.download.failed"));
    }
    let cores = guard
        .cores()
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut command = tokio::process::Command::new("taskset");
    command
        .args(["--cpu-list", &cores, "yt-dlp"])
        .kill_on_drop(true);
    Ok((guard, command))
}

pub(crate) fn size_error(bytes: u64) -> Option<String> {
    let max = super::single::MAX_SIZE_MB;
    (bytes > max * 1024 * 1024)
        .then(|| crate::i18n::tf("youtube.audio.oversized", &[("max", &max.to_string())]))
}
