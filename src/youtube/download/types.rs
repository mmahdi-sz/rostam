use std::sync::{Arc, Mutex};

use super::super::types::{AudioLanguage, SubtitleLanguage, VideoCodec, VideoFormatOption};

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
