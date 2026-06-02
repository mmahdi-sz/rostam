#[derive(Debug)]
pub struct VideoInfo {
    pub title: String,
    pub channel: String,
    pub duration: Option<u64>,
    pub view_count: Option<u64>,
    pub like_count: Option<u64>,
    pub upload_date: Option<String>,
    pub thumbnail: Option<String>,
    pub webpage_url: String,
    pub description: Option<String>,
    pub available_heights: Vec<u32>,
    pub video_formats: Vec<VideoFormatOption>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VideoCodec {
    H264,
    H265,
    Vp9,
    Av1,
}

impl VideoCodec {
    pub fn key(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::H265 => "h265",
            Self::Vp9 => "vp9",
            Self::Av1 => "av1",
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::H264 => "youtube.codec.buttons.h264",
            Self::H265 => "youtube.codec.buttons.h265",
            Self::Vp9 => "youtube.codec.buttons.vp9",
            Self::Av1 => "youtube.codec.buttons.av1",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "h264" => Some(Self::H264),
            "h265" => Some(Self::H265),
            "vp9" => Some(Self::Vp9),
            "av1" => Some(Self::Av1),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VideoFormatOption {
    pub height: u32,
    pub codec: VideoCodec,
    pub format_id: String,
}

#[derive(Debug)]
pub enum FetchError {
    RateLimited,
    BadCookie(String),
    Other(String),
}
