use crate::i18n::tf;
use crate::stats::{get_user_stats, get_download_stats, fmt_bytes};
use tokio_postgres::Client;

pub async fn render_stats(client: &Client) -> String {
    let users = match get_user_stats(client).await {
        Ok(u) => u,
        Err(e) => {
            eprintln!("[admin event=stats_users_failed] err={e}");
            return "خطا در دریافت آمار".to_string();
        }
    };
    let dl = match get_download_stats(client).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[admin event=stats_downloads_failed] err={e}");
            return "خطا در دریافت آمار دانلود".to_string();
        }
    };
    tf("admin.stats_title", &[
        ("total",   &users.total.to_string()),
        ("new_1d",  &users.new_1d.to_string()),
        ("new_3d",  &users.new_3d.to_string()),
        ("new_7d",  &users.new_7d.to_string()),
        ("new_30d", &users.new_30d.to_string()),

        ("req_1d",  &dl.requests_1d.to_string()),
        ("req_3d",  &dl.requests_3d.to_string()),
        ("req_7d",  &dl.requests_7d.to_string()),
        ("req_30d", &dl.requests_30d.to_string()),

        ("dl_1d",   &fmt_bytes(dl.bytes_downloaded_1d)),
        ("dl_3d",   &fmt_bytes(dl.bytes_downloaded_3d)),
        ("dl_7d",   &fmt_bytes(dl.bytes_downloaded_7d)),
        ("dl_30d",  &fmt_bytes(dl.bytes_downloaded_30d)),

        ("up_1d",   &dl.uploads_ok_1d.to_string()),
        ("up_3d",   &dl.uploads_ok_3d.to_string()),
        ("up_7d",   &dl.uploads_ok_7d.to_string()),
        ("up_30d",  &dl.uploads_ok_30d.to_string()),

        ("upb_1d",  &fmt_bytes(dl.bytes_uploaded_1d)),
        ("upb_3d",  &fmt_bytes(dl.bytes_uploaded_3d)),
        ("upb_7d",  &fmt_bytes(dl.bytes_uploaded_7d)),
        ("upb_30d", &fmt_bytes(dl.bytes_uploaded_30d)),
    ])
}
