mod types;
mod extract;
mod fetch;
mod format;
pub mod jalali;
mod quality_keyboard;
mod handle;

pub use types::{VideoInfo, FetchError};
pub use extract::extract_youtube_urls;
pub use fetch::fetch_video_info;
pub use format::{
    escape_markdown_v2, format_duration, format_count,
    format_upload_date, build_caption, build_description_blockquotes,
};
pub use handle::handle_youtube_url;
pub use quality_keyboard::handle_quality_callback;
pub use jalali::gregorian_to_jalali;
