//! ID3 metadata & cover art embedding for downloaded MP3 files.

use crate::spotify::client::SpotifyTrackMeta;
use id3::frame::{Picture, PictureType};
use id3::{Tag, TagLike};
use std::path::{Path, PathBuf};

pub async fn apply_id3_tags(
    mp3_path: &Path,
    meta: &SpotifyTrackMeta,
    cover_stem: &str,
    trace_id: u64,
) -> anyhow::Result<Option<PathBuf>> {
    let mut tag = Tag::new();
    tag.set_title(&meta.title);
    tag.set_artist(&meta.artists_joined);
    tag.set_album(&meta.album_name);

    let mut saved_cover_path: Option<PathBuf> = None;

    if let Some(cover_url) = &meta.cover_url {
        match reqwest::get(cover_url).await {
            Ok(res) if res.status().is_success() => {
                if let Ok(bytes) = res.bytes().await {
                    let mime = if cover_url.contains(".png") {
                        "image/png".to_string()
                    } else {
                        "image/jpeg".to_string()
                    };
                    let picture = Picture {
                        mime_type: mime,
                        picture_type: PictureType::CoverFront,
                        description: "Spotify Cover".to_string(),
                        data: bytes.to_vec(),
                    };
                    tag.add_frame(picture);

                    if let Some(parent) = mp3_path.parent() {
                        let cover_file = parent.join(format!("{cover_stem}.jpg"));
                        if tokio::fs::write(&cover_file, &bytes).await.is_ok() {
                            saved_cover_path = Some(cover_file);
                        }
                    }

                    log_ev!(
                        "sp",
                        trace_id,
                        "id3_cover_embedded",
                        "bytes" => bytes.len()
                    );
                }
            }
            Ok(res) => {
                log_ev!(
                    "sp",
                    trace_id,
                    "id3_cover_http_err",
                    "status" => res.status().as_u16()
                );
            }
            Err(e) => {
                log_ev!("sp", trace_id, "id3_cover_fetch_err", "err" => e.to_string());
            }
        }
    }

    tag.write_to_path(mp3_path, id3::Version::Id3v24)?;
    log_ev!(
        "sp",
        trace_id,
        "id3_tags_saved",
        "path" => mp3_path.display().to_string()
    );
    Ok(saved_cover_path)
}
