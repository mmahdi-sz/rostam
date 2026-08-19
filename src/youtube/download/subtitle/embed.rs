use crate::youtube::trace::log_trace;
use super::files::subtitle_matches_selection;

pub async fn embed_subtitles(
    dir: &std::path::Path,
    video_path: &str,
    target_langs: &[String],
    trace_id: u64,
) -> Result<String, String> {
    log_trace(trace_id, "embed_subtitles_started", "");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(video_path.to_string());
    };
    let mut srts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_srt = path.extension().and_then(|e| e.to_str()) == Some("srt");
        let matches_selection = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| subtitle_matches_selection(n, target_langs))
            .unwrap_or(false);
        if is_srt && matches_selection {
            srts.push(path);
        }
    }

    if srts.is_empty() {
        return Ok(video_path.to_string());
    }

    srts.sort_by(|a, b| {
        let a_name = a
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let b_name = b
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let a_is_fa = a_name.contains("translated_fa")
            || a_name.contains(".fa.")
            || a_name.contains("_fa.srt");
        let b_is_fa = b_name.contains("translated_fa")
            || b_name.contains(".fa.")
            || b_name.contains("_fa.srt");
        match (a_is_fa, b_is_fa) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a_name.cmp(&b_name),
        }
    });

    let is_mkv = video_path.ends_with(".mkv");
    let ext = if is_mkv { "mkv" } else { "mp4" };
    let sub_codec = if is_mkv { "srt" } else { "mov_text" };
    let stem = video_path
        .strip_suffix(&format!(".{ext}"))
        .unwrap_or(video_path);
    let out_path = format!("{stem}_embedded.{ext}");

    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.arg("-y").arg("-i").arg(video_path);

    for srt in &srts {
        cmd.arg("-i").arg(srt.to_string_lossy().as_ref());
    }

    cmd.arg("-c").arg("copy");
    cmd.arg("-c:s").arg(sub_codec);

    cmd.arg("-map").arg("0");
    for (i, srt) in srts.iter().enumerate() {
        let idx = i + 1;
        cmd.arg("-map").arg(idx.to_string());

        let fname = srt
            .file_name()
            .unwrap_or_default()
            .to_str()
            .unwrap_or_default()
            .to_lowercase();
        let (lang_code, lang_title) = if fname.contains("translated_fa")
            || fname.contains(".fa.")
            || fname.contains("_fa.srt")
        {
            ("per", "زیرنویس فارسی (Farsi)")
        } else if fname.contains(".en.")
            || fname.contains("_en.srt")
            || fname.contains("translated_en")
        {
            ("eng", "English Subtitle")
        } else {
            ("und", "Subtitle")
        };

        cmd.arg(format!("-metadata:s:s:{i}"))
            .arg(format!("language={lang_code}"));
        cmd.arg(format!("-metadata:s:s:{i}"))
            .arg(format!("title={lang_title}"));
        cmd.arg(format!("-metadata:s:s:{i}"))
            .arg(format!("handler_name={lang_title}"));

        if i == 0 {
            cmd.arg("-disposition:s:0").arg("default");
        } else {
            cmd.arg(format!("-disposition:s:{i}")).arg("0");
        }
    }
    cmd.arg(&out_path);

    let out = cmd.output().await.map_err(|e| e.to_string())?;
    if out.status.success() {
        let _ = tokio::fs::remove_file(video_path).await;
        for srt in &srts {
            let _ = tokio::fs::remove_file(srt).await;
        }
        if !is_mkv {
            fix_embedded_subtitle_flags(&out_path, trace_id).await;
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
    log_trace(
        trace_id,
        "subtitle_flags_fix",
        &format!("path={video_path}"),
    );
    let path = video_path.to_string();
    let res = tokio::task::spawn_blocking(move || patch_first_tx3g_display_flags(&path)).await;
    match res {
        Ok(Ok(Some(offset))) => {
            log_trace(
                trace_id,
                "subtitle_flags_fix",
                &format!("=> ok offset={offset}"),
            );
            true
        }
        Ok(Ok(None)) => {
            log_trace(
                trace_id,
                "subtitle_flags_fix",
                "=> pass no enabled tx3g track",
            );
            false
        }
        Ok(Err(e)) => {
            log_trace(trace_id, "subtitle_flags_fix", &format!("=> fail err={e}"));
            false
        }
        Err(e) => {
            log_trace(
                trace_id,
                "subtitle_flags_fix",
                &format!("=> fail join err={e}"),
            );
            false
        }
    }
}

/// tx3g displayFlags bits 0x80000000|0x40000000 = "all/some samples forced".
pub const TX3G_FORCED_FLAGS: [u8; 4] = [0xC0, 0x00, 0x00, 0x00];

/// Finds the first child box named `name` in `buf[off..end]`.
pub fn find_box(buf: &[u8], mut off: usize, end: usize, name: &[u8; 4]) -> Option<(usize, usize)> {
    while off + 8 <= end.min(buf.len()) {
        let size_bytes: [u8; 4] = buf[off..off + 4].try_into().ok()?;
        let size = u32::from_be_bytes(size_bytes) as usize;
        if size < 8 || off + size > end {
            return None;
        }
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
pub fn patch_first_tx3g_display_flags(path: &str) -> std::io::Result<Option<u64>> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
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
        if size < 8 {
            break;
        }
        if &hdr[4..8] == b"moov" {
            moov = Some((off, size));
            break;
        }
        off += size;
    }
    let Some((moov_off, moov_size)) = moov else {
        return Ok(None);
    };
    // moov of a 2GB video is a few MB; anything huge means a corrupt size field.
    if moov_size > 128 * 1024 * 1024 {
        return Ok(None);
    }

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
        if !enabled {
            continue;
        }

        let Some((mdia, mdia_size)) = find_box(&buf, trak + 8, trak_end, b"mdia") else {
            continue;
        };
        let Some((minf, minf_size)) = find_box(&buf, mdia + 8, mdia + mdia_size, b"minf") else {
            continue;
        };
        let Some((stbl, stbl_size)) = find_box(&buf, minf + 8, minf + minf_size, b"stbl") else {
            continue;
        };
        let Some((stsd, stsd_size)) = find_box(&buf, stbl + 8, stbl + stbl_size, b"stsd") else {
            continue;
        };

        // stsd: 8 header + 4 version/flags + 4 entry_count, then first entry.
        let entry = stsd + 16;
        if entry + 20 > stsd + stsd_size {
            continue;
        }
        if &buf[entry + 4..entry + 8] != b"tx3g" {
            continue;
        }

        // tx3g entry: 4 size + 4 type + 6 reserved + 2 data_ref_index, then displayFlags.
        let abs = moov_off + (entry + 16) as u64;
        f.seek(SeekFrom::Start(abs))?;
        f.write_all(&TX3G_FORCED_FLAGS)?;
        return Ok(Some(abs));
    }
    Ok(None)
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
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=black:size=64x64:rate=10:duration=3",
                "-i",
                srt.to_str().unwrap(),
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-c:s",
                "mov_text",
                "-map",
                "0",
                "-map",
                "1",
                out.to_str().unwrap(),
            ])
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
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=black:size=64x64:rate=10:duration=1",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                out.to_str().unwrap(),
            ])
            .status()
            .expect("ffmpeg must exist on host");
        assert!(st.success());

        let before = std::fs::read(&out).unwrap();
        let res = patch_first_tx3g_display_flags(out.to_str().unwrap()).unwrap();
        assert!(res.is_none());
        assert_eq!(
            before,
            std::fs::read(&out).unwrap(),
            "file must be untouched"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
