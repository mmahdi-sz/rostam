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
}

#[derive(Debug)]
pub enum FetchError {
    RateLimited,
    BadCookie(String),
    Other(String),
}
