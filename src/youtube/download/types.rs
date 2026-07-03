use std::sync::{Arc, Mutex};

use super::super::types::{AudioLanguage, SubtitleLanguage, VideoCodec, VideoFormatOption};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SubtitleMode {
    File,
    Embedded,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioQuality {
    Best,
    Medium,
    Low,
}

impl AudioQuality {
    pub fn format_spec(self) -> &'static str {
        match self {
            Self::Best   => "bestaudio",
            Self::Medium => "bestaudio[abr<=128]/bestaudio",
            Self::Low    => "bestaudio[abr<=64]/bestaudio",
        }
    }
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Best   => "youtube.audio.quality.best",
            Self::Medium => "youtube.audio.quality.medium",
            Self::Low    => "youtube.audio.quality.low",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "best" => Some(Self::Best),
            "mid"  => Some(Self::Medium),
            "low"  => Some(Self::Low),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Best   => "best",
            Self::Medium => "mid",
            Self::Low    => "low",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub height: u32,
    pub codec: VideoCodec,
    pub audio_lang: Option<String>,
    pub subtitle_langs: Vec<String>,
    pub subtitle_mode: SubtitleMode,
    pub view: SelectionView,
    pub audio_only: Option<AudioQuality>,
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
    pub channel: String,
    pub duration: Option<u64>,
    pub thumbnail_url: Option<String>,
    pub formats: Vec<VideoFormatOption>,
    pub audio_languages: Vec<AudioLanguage>,
    pub subtitle_languages: Vec<SubtitleLanguage>,
    pub selection: Arc<Mutex<Option<Selection>>>,
}
