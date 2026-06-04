use std::process::Stdio;
use std::time::Duration;

use frankenstein::client_reqwest::Bot;
use rand::seq::SliceRandom;
use tokio::process::Command;
use tokio::time::sleep;

use crate::bot::send_text;

pub struct CookieRefresherConfig {
    /// Real Firefox profile dir — used for `firefox --profile` and yt-dlp login check.
    pub profile_path: String,
    pub profile_name: String,
    /// Cache dir to copy fresh cookies into after Firefox closes (may equal profile_path if no cache).
    pub cache_dir: String,
    pub links_file: String,
    pub duration_secs: u64,
    pub link_count: usize,
    pub admin_chat_id: i64,
}

impl Default for CookieRefresherConfig {
    fn default() -> Self {
        Self {
            profile_path: String::new(),
            profile_name: String::new(),
            cache_dir: String::new(),
            links_file: "files/youtube_links.txt".to_string(),
            duration_secs: 3600,
            link_count: 3,
            admin_chat_id: 0,
        }
    }
}

pub async fn run(api: &Bot, config: CookieRefresherConfig) -> Result<(), String> {
    println!("[cookie_refresher] starting for profile={}", config.profile_name);

    let links = load_links(&config)?;
    if links.is_empty() {
        let msg = format!("⚠️ فایل لینک‌های {} خالیه یا پیدا نشد!", config.profile_name);
        notify(api, config.admin_chat_id, &msg).await;
        return Err("no links available".to_string());
    }

    if !check_login(&config).await? {
        let msg = format!("⚠️ اکانت {} لاگین نشده!", config.profile_name);
        notify(api, config.admin_chat_id, &msg).await;
        return Err(format!("profile {} is not logged in", config.profile_name));
    }

    println!("[cookie_refresher] login ok, opening firefox with {} links", links.len());

    let pid = open_firefox(&config.profile_path, &links).await?;

    let crashed = wait_or_crash(pid, config.duration_secs).await;

    kill_firefox(pid).await;

    if crashed {
        let msg = format!("⚠️ فایرفاکس اکانت {} قبل از اتمام زمان crash کرد!", config.profile_name);
        notify(api, config.admin_chat_id, &msg).await;
        return Err(format!("firefox crashed for profile {}", config.profile_name));
    }

    if !config.cache_dir.is_empty() && config.cache_dir != config.profile_path {
        refresh_cache(&config.profile_path, &config.cache_dir, &config.profile_name);
    }

    let msg = format!("✅ کوکی‌های {} با موفقیت آپدیت شدن", config.profile_name);
    notify(api, config.admin_chat_id, &msg).await;
    println!("[cookie_refresher] done for profile={}", config.profile_name);
    Ok(())
}

fn load_links(config: &CookieRefresherConfig) -> Result<Vec<String>, String> {
    let contents = std::fs::read_to_string(&config.links_file)
        .map_err(|e| format!("failed to read links file {}: {e}", config.links_file))?;

    let mut all: Vec<String> = contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect();

    if all.is_empty() {
        return Ok(vec![]);
    }

    let mut rng = rand::thread_rng();
    all.shuffle(&mut rng);
    all.truncate(config.link_count);
    Ok(all)
}

async fn check_login(config: &CookieRefresherConfig) -> Result<bool, String> {
    println!("[cookie_refresher] checking login for profile={}", config.profile_name);
    let output = Command::new("yt-dlp")
        .arg("--cookies-from-browser")
        .arg(format!("firefox:{}", config.profile_path))
        .arg("--print")
        .arg("%(uploader)s")
        .arg("--no-download")
        .arg("--no-warnings")
        .arg("https://www.youtube.com/feed/subscriptions")
        .output()
        .await
        .map_err(|e| format!("failed to spawn yt-dlp: {e}"))?;

    if !output.status.success() {
        println!("[cookie_refresher] yt-dlp login check failed status={}", output.status);
        return Ok(false);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        println!("[cookie_refresher] yt-dlp login check: empty output");
        return Ok(false);
    }
    println!("[cookie_refresher] login check ok uploader={trimmed:?}");
    Ok(true)
}

async fn open_firefox(profile_path: &str, links: &[String]) -> Result<u32, String> {
    if links.is_empty() {
        return Err("no links to open".to_string());
    }

    let child = Command::new("firefox")
        .arg("--profile")
        .arg(profile_path)
        .arg(&links[0])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn firefox: {e}"))?;

    let pid = child.id().ok_or("firefox has no pid")?;
    println!("[cookie_refresher] firefox opened pid={pid} first_url={}", links[0]);

    for url in &links[1..] {
        sleep(Duration::from_secs(1)).await;
        let _ = Command::new("firefox")
            .arg("--profile")
            .arg(profile_path)
            .arg("--new-tab")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        println!("[cookie_refresher] opened new tab url={url}");
    }

    Ok(pid)
}

async fn wait_or_crash(pid: u32, duration_secs: u64) -> bool {
    let deadline = sleep(Duration::from_secs(duration_secs));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => {
                println!("[cookie_refresher] duration elapsed pid={pid}");
                return false;
            }
            _ = sleep(Duration::from_secs(5)) => {
                if !is_running(pid) {
                    println!("[cookie_refresher] firefox pid={pid} is no longer running (crash)");
                    return true;
                }
            }
        }
    }
}

fn is_running(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

async fn kill_firefox(pid: u32) {
    println!("[cookie_refresher] killing firefox pid={pid}");
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .output()
        .await;
    sleep(Duration::from_secs(3)).await;
    if is_running(pid) {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(pid.to_string())
            .output()
            .await;
    }
}

fn refresh_cache(source_dir: &str, cache_dir: &str, profile_name: &str) {
    println!("[cookie_refresher] copying cookies {source_dir} → {cache_dir} profile={profile_name}");
    for name in ["cookies.sqlite", "cookies.sqlite-wal", "cookies.sqlite-shm"] {
        let src = std::path::Path::new(source_dir).join(name);
        if !src.exists() { continue; }
        let dst = std::path::Path::new(cache_dir).join(name);
        match std::fs::copy(&src, &dst) {
            Ok(_) => println!("[cookie_refresher] copied {name} profile={profile_name}"),
            Err(e) => eprintln!("[cookie_refresher] failed to copy {name} profile={profile_name}: {e}"),
        }
    }
}

async fn notify(api: &Bot, chat_id: i64, text: &str) {
    println!("[cookie_refresher] notify chat_id={chat_id} msg={text:?}");
    if chat_id == 0 {
        return;
    }
    if let Err(e) = send_text(api, chat_id, text).await {
        eprintln!("[cookie_refresher] failed to send notify: {e}");
    }
}
