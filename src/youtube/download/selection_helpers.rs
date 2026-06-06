use super::super::types::{AudioLanguage, VideoCodec};
use super::types::{Selection, SelectionView, SubtitleMode, YoutubeRequest};

const CODEC_PRIORITY: &[VideoCodec] = &[
    VideoCodec::Av1,
    VideoCodec::Vp9,
    VideoCodec::H265,
    VideoCodec::H264,
];

pub fn codecs_for_height(req: &YoutubeRequest, height: u32) -> Vec<VideoCodec> {
    let mut out = Vec::new();
    for f in &req.formats {
        if f.height == height && !out.contains(&f.codec) {
            out.push(f.codec);
        }
    }
    out
}

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

pub fn find_format<'a>(
    req: &'a YoutubeRequest,
    height: u32,
    codec: VideoCodec,
) -> Option<&'a super::super::types::VideoFormatOption> {
    req.formats
        .iter()
        .find(|f| f.height == height && f.codec == codec)
}
