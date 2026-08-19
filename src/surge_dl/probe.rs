use std::path::Path;
use std::time::Duration;

pub fn available_disk_space(path: &str) -> std::io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let path_obj = Path::new(path);
    let c_path = CString::new(path_obj.as_os_str().as_bytes())?;
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stat) == 0 {
            let free_bytes = (stat.f_bavail as u64) * (stat.f_frsize as u64);
            Ok(free_bytes)
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

pub fn detect_social_platform(text: &str) -> Option<&'static str> {
    let text = text.trim();
    if !crate::validation::is_safe_url(text) {
        return None;
    }
    let Ok(parsed) = reqwest::Url::parse(text) else {
        return None;
    };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?.to_lowercase();
    if host == "youtube.com"
        || host.ends_with(".youtube.com")
        || host == "youtu.be"
        || host.ends_with(".youtu.be")
    {
        return Some("youtube");
    }
    if host == "t.me"
        || host.ends_with(".t.me")
        || host == "telegram.org"
        || host.ends_with(".telegram.org")
        || host == "telegram.me"
        || host.ends_with(".telegram.me")
    {
        return Some("telegram");
    }
    if host == "instagram.com"
        || host.ends_with(".instagram.com")
        || host == "instagr.am"
        || host.ends_with(".instagr.am")
    {
        return Some("instagram");
    }
    if host == "tiktok.com" || host.ends_with(".tiktok.com") || host == "vt.tiktok.com" {
        return Some("tiktok");
    }
    if host == "soundcloud.com" || host.ends_with(".soundcloud.com") || host == "on.soundcloud.com"
    {
        return Some("soundcloud");
    }
    if host == "twitter.com"
        || host.ends_with(".twitter.com")
        || host == "x.com"
        || host.ends_with(".x.com")
    {
        return Some("twitter");
    }
    if host == "pinterest.com"
        || host.ends_with(".pinterest.com")
        || host == "pin.it"
        || host.ends_with(".pin.it")
    {
        return Some("pinterest");
    }
    if host == "facebook.com"
        || host.ends_with(".facebook.com")
        || host == "fb.watch"
        || host == "fb.com"
    {
        return Some("facebook");
    }
    if host == "threads.net" || host.ends_with(".threads.net") {
        return Some("threads");
    }
    if host == "soundcloud.com" || host.ends_with(".soundcloud.com") {
        return Some("soundcloud");
    }
    if host == "spotify.com" || host.ends_with(".spotify.com") {
        return Some("spotify");
    }
    if host == "aparat.com" || host.ends_with(".aparat.com") {
        return Some("aparat");
    }
    if host == "rubika.ir"
        || host.ends_with(".rubika.ir")
        || host == "rubika.com"
        || host.ends_with(".rubika.com")
    {
        return Some("rubika");
    }
    if host == "eitaa.com" || host.ends_with(".eitaa.com") {
        return Some("eitaa");
    }
    // Play Store pages are HTML, not files — probing one yielded
    // `name=details size=0` and offered it as a "download". Classify it so the
    // dispatcher shows the unsupported-platform notice instead.
    if host == "play.google.com" || host == "play.app.goo.gl" {
        return Some("playstore");
    }
    None
}

pub fn is_direct_link(text: &str) -> bool {
    let text = text.trim();
    if !crate::validation::is_safe_url(text) {
        return false;
    }

    let Ok(parsed) = reqwest::Url::parse(text) else {
        return false;
    };

    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return false;
    }

    if let Some(host) = parsed.host_str() {
        let host = host.to_lowercase();
        if host == "t.me"
            || host.ends_with(".t.me")
            || host == "telegram.org"
            || host.ends_with(".telegram.org")
            || host == "telegram.me"
            || host.ends_with(".telegram.me")
        {
            return false;
        }
    } else {
        return false;
    }

    true
}

/// Reduces user rename input to a bare filename, rejecting anything that could
/// escape the download dir (path separators, `.`/`..`). Returns None if nothing
/// safe remains — the caller then aborts the rename.
pub(crate) fn sanitize_rename(typed: &str) -> Option<String> {
    std::path::Path::new(typed)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && *n != "." && *n != "..")
        .map(|n| {
            let s: String = n.chars().take(200).collect();
            s
        })
}

pub(crate) fn safe_download_filename(name: &str) -> String {
    let cleaned = name
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
        .take(255)
        .collect::<String>();
    let cleaned = cleaned.trim().trim_matches('.');
    if cleaned.is_empty() {
        "file".to_string()
    } else {
        cleaned.to_string()
    }
}

pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

pub(crate) fn filename_from_url(url: &str) -> String {
    let no_query = url.split('?').next().unwrap_or(url);
    let name = no_query
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("file");
    safe_download_filename(&percent_decode(name))
}

pub(crate) fn extract_content_disposition_filename(header: &str) -> Option<String> {
    for part in header.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("filename*=") {
            let val = v.trim_matches('"');
            if let Some(idx) = val.find("''") {
                let encoded = &val[idx + 2..];
                return Some(percent_decode(encoded));
            }
        }
    }
    for part in header.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("filename=") {
            return Some(v.trim_matches('"').to_string());
        }
    }
    None
}

/// Probes filename and size via HEAD request before download.
pub(crate) async fn probe_url(url: &str) -> (String, Option<u64>) {
    let fallback = filename_from_url(url);
    // User-Agent prevents 403 error page size from being misreported as file size.
    let resp = match crate::http::client()
        .head(url)
        .header(reqwest::header::USER_AGENT, "Mozilla/5.0")
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => return (fallback, None),
    };
    let filename = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_content_disposition_filename)
        .map(|s| safe_download_filename(&percent_decode(&s)))
        .unwrap_or(fallback);
    let size = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    (filename, size)
}

#[cfg(test)]
mod tests {
    use super::{
        available_disk_space, detect_social_platform, extract_content_disposition_filename,
        is_direct_link, safe_download_filename, sanitize_rename,
    };

    #[test]
    fn test_play_store_is_an_unsupported_platform_not_a_file() {
        assert_eq!(
            detect_social_platform("https://play.google.com/store/apps/details?id=com.example"),
            Some("playstore")
        );
        assert_eq!(detect_social_platform("https://example.com/file.zip"), None);
    }

    #[test]
    fn test_is_direct_link_ignores_telegram_urls() {
        assert!(!is_direct_link("https://t.me/c/3310766784/162"));
        assert!(!is_direct_link("https://user@t.me/c/3310766784/162"));
        assert!(!is_direct_link("https://telegram.org/blog"));
        assert!(!is_direct_link("http://telegram.me/user"));
        assert!(is_direct_link("https://example.com/file.zip"));
        assert!(is_direct_link("http://direct.download.com/video.mp4"));
    }

    #[test]
    fn direct_link_rejects_shell_like_and_private_targets() {
        assert!(!is_direct_link("https://test123 ; sleep 10"));
        assert!(!is_direct_link("http://$(sleep 10)"));
        assert!(!is_direct_link("http://127.0.0.1/private"));
        assert!(!is_direct_link("http://169.254.169.254/latest/meta-data"));
    }

    #[test]
    fn download_filename_drops_shell_metacharacters_and_paths() {
        assert_eq!(safe_download_filename("sleep 15; .pdf"), "sleep 15 .pdf");
        assert_eq!(safe_download_filename("../../etc/passwd"), "etcpasswd");
        assert_eq!(safe_download_filename("$()`|&"), "file");
    }

    #[test]
    fn keeps_plain_names() {
        assert_eq!(sanitize_rename("movie.mp4").as_deref(), Some("movie.mp4"));
        assert_eq!(sanitize_rename("my file").as_deref(), Some("my file"));
    }

    #[test]
    fn test_sanitize_rename_multibyte_utf8() {
        let long_farsi = "نام_فایل_بسیار_طولانی_برای_تست_سیستم_دانلود_که_نباید_در_زمان_برش_بایتی_باعث_پنیک_در_رست_شود_چون_کاراکترهای_فارسی_چندبایتی_هستند_و_برش_روی_مرز_بایت_نامعتبر_موجب_کرش_پروسه_میگردد.mp4";
        let sanitized = sanitize_rename(long_farsi);
        assert!(sanitized.is_some());
        let res = sanitized.unwrap();
        assert!(res.chars().count() <= 200);
    }

    #[test]
    fn test_extract_content_disposition_filename() {
        assert_eq!(
            extract_content_disposition_filename("attachment; filename=\"test.mp4\""),
            Some("test.mp4".to_string())
        );
        assert_eq!(
            extract_content_disposition_filename(
                "attachment; filename*=UTF-8''%D9%81%D8%A7%DB%8C%D9%84.mp4"
            ),
            Some("فایل.mp4".to_string())
        );
    }

    #[test]
    fn test_available_disk_space() {
        let space = available_disk_space("/tmp");
        assert!(space.is_ok());
        assert!(space.unwrap() > 0);
    }

    #[test]
    fn strips_traversal() {
        // Path separators and parent refs are stripped to the trailing component…
        assert_eq!(
            sanitize_rename("../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(sanitize_rename("/etc/cron.d/x").as_deref(), Some("x"));
        // …and inputs that reduce to nothing safe are rejected outright.
        assert_eq!(sanitize_rename(".."), None);
        assert_eq!(sanitize_rename("../.."), None);
        assert_eq!(sanitize_rename("/"), None);
    }
}
