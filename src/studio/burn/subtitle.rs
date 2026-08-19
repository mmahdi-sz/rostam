//! Subtitle format detection, styling, escaping, and conversion.

use std::path::Path;

pub const DEFAULT_SUBTITLE_FONT: &str = "Arial";
pub const DEFAULT_SUBTITLE_FONTSIZE: u32 = 18;
pub const DEFAULT_SUBTITLE_PRIMARY_COLOR: &str = "&H00FFFFFF";
pub const DEFAULT_SUBTITLE_OUTLINE_COLOR: &str = "&H00000000";
pub const DEFAULT_SUBTITLE_BORDER_STYLE: u32 = 1;
pub const DEFAULT_SUBTITLE_OUTLINE: u32 = 2;
pub const DEFAULT_SUBTITLE_SHADOW: u32 = 1;
pub const DEFAULT_SUBTITLE_ALIGNMENT: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    Srt,
    Ass,
    Vtt,
}

impl SubtitleFormat {
    /// Extension used for the copy stored inside the work dir.
    pub fn ext(&self) -> &'static str {
        match self {
            SubtitleFormat::Srt => "srt",
            SubtitleFormat::Ass => "ass",
            SubtitleFormat::Vtt => "vtt",
        }
    }
}

pub fn detect_subtitle_format(filename: &str) -> Option<SubtitleFormat> {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "srt" => Some(SubtitleFormat::Srt),
        "ass" | "ssa" => Some(SubtitleFormat::Ass),
        "vtt" => Some(SubtitleFormat::Vtt),
        _ => None,
    }
}

pub fn build_force_style_arg() -> String {
    format!(
        "Fontname={DEFAULT_SUBTITLE_FONT},Fontsize={DEFAULT_SUBTITLE_FONTSIZE},\
         PrimaryColour={DEFAULT_SUBTITLE_PRIMARY_COLOR},OutlineColour={DEFAULT_SUBTITLE_OUTLINE_COLOR},\
         BorderStyle={DEFAULT_SUBTITLE_BORDER_STYLE},Outline={DEFAULT_SUBTITLE_OUTLINE},\
         Shadow={DEFAULT_SUBTITLE_SHADOW},Alignment={DEFAULT_SUBTITLE_ALIGNMENT}"
    )
}

/// Escapes one value for an *unquoted* ffmpeg filtergraph argument. ffmpeg parses the
/// filtergraph itself, so shell quoting rules do not apply: every character that would
/// terminate or split the argument takes a single backslash.
pub fn escape_filter_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '\\' | '\'' | ':' | ',' | ';' | '=' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub fn escape_ffmpeg_filter_path(path: &Path) -> String {
    escape_filter_value(&path.to_string_lossy())
}

/// Builds the `-vf` argument: ASS keeps its own styling, SRT/VTT get the forced default style.
pub fn build_filter_arg(format: SubtitleFormat, sub_path: &Path) -> String {
    let path = escape_ffmpeg_filter_path(sub_path);
    match format {
        SubtitleFormat::Ass => format!("ass=filename={path}"),
        SubtitleFormat::Srt | SubtitleFormat::Vtt => format!(
            "subtitles=filename={path}:force_style={}",
            escape_filter_value(&build_force_style_arg())
        ),
    }
}

pub async fn convert_vtt_to_srt(vtt_path: &Path, srt_path: &Path) -> anyhow::Result<()> {
    let output = tokio::process::Command::new(crate::config::ffmpeg_path())
        .args(["-y", "-hide_banner", "-nostdin", "-i"])
        .arg(vtt_path)
        .arg(srt_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("failed to execute ffmpeg for vtt conversion: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("vtt conversion failed: {err}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_subtitle_format() {
        assert_eq!(
            detect_subtitle_format("movie.srt"),
            Some(SubtitleFormat::Srt)
        );
        assert_eq!(
            detect_subtitle_format("movie.ASS"),
            Some(SubtitleFormat::Ass)
        );
        assert_eq!(
            detect_subtitle_format("movie.ssa"),
            Some(SubtitleFormat::Ass)
        );
        assert_eq!(
            detect_subtitle_format("movie.vtt"),
            Some(SubtitleFormat::Vtt)
        );
        assert_eq!(detect_subtitle_format("movie.txt"), None);
    }

    #[test]
    fn test_escape_ffmpeg_filter_path_uses_filtergraph_rules() {
        // Not shell rules: ffmpeg parses the filtergraph itself, so a single backslash is correct.
        let p = Path::new("/tmp/dir:with_colon/sub'file.ass");
        assert_eq!(
            escape_ffmpeg_filter_path(p),
            "/tmp/dir\\:with_colon/sub\\'file.ass"
        );
        assert_eq!(escape_filter_value("a,b=c[d];e"), "a\\,b\\=c\\[d\\]\\;e");
        assert_eq!(escape_filter_value("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn test_build_filter_arg_escapes_force_style() {
        let ass = build_filter_arg(SubtitleFormat::Ass, Path::new("/tmp/w/sub.ass"));
        assert_eq!(ass, "ass=filename=/tmp/w/sub.ass");

        let srt = build_filter_arg(SubtitleFormat::Srt, Path::new("/tmp/w/sub.srt"));
        assert!(srt.starts_with("subtitles=filename=/tmp/w/sub.srt:force_style="));
        // Style separators must be escaped or ffmpeg reads them as extra filter options.
        let style = srt.split("force_style=").nth(1).unwrap();
        assert!(!style.contains("\\\\,"), "no double-escaping");
        assert!(srt.contains("\\,Fontsize"));
        assert!(srt.contains("Fontname\\=Arial"));
    }

    #[test]
    fn test_subtitle_ext_is_fixed_and_safe() {
        // Work-dir names never derive from user input, so traversal cannot happen.
        assert_eq!(SubtitleFormat::Srt.ext(), "srt");
        assert_eq!(SubtitleFormat::Ass.ext(), "ass");
        assert_eq!(SubtitleFormat::Vtt.ext(), "vtt");
    }
}
