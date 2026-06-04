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
    let p = &config.profile_name;
    println!("[cookie_refresh profile={p} event=start] links_file={} duration={}s", config.links_file, config.duration_secs);

    let links = load_links(&config)?;
    if links.is_empty() {
        println!("[cookie_refresh profile={p} event=no_links] links_file={}", config.links_file);
        let msg = format!("⚠️ فایل لینک‌های {} خالیه یا پیدا نشد!", p);
        notify(api, config.admin_chat_id, &msg).await;
        return Err("no links available".to_string());
    }

    if !check_login(&config)? {
        let msg = format!("⚠️ اکانت {} لاگین نشده!", p);
        notify(api, config.admin_chat_id, &msg).await;
        return Err(format!("profile {} is not logged in", p));
    }

    println!("[cookie_refresh profile={p} event=firefox_starting] link_count={}", links.len());

    kill_existing_firefox(&config.profile_path, p).await;
    let pid = open_firefox(&config.profile_path, p, &links).await?;

    let crashed = wait_or_crash(pid, config.duration_secs, p).await;

    kill_firefox(pid, p).await;

    if crashed {
        println!("[cookie_refresh profile={p} event=done] success=false reason=crashed");
        let msg = format!("⚠️ فایرفاکس اکانت {} قبل از اتمام زمان crash کرد!", p);
        notify(api, config.admin_chat_id, &msg).await;
        return Err(format!("firefox crashed for profile {}", p));
    }

    if !config.cache_dir.is_empty() && config.cache_dir != config.profile_path {
        refresh_cache(&config.profile_path, &config.cache_dir, p);
    }

    println!("[cookie_refresh profile={p} event=done] success=true");
    let msg = format!("✅ کوکی‌های {} با موفقیت آپدیت شدن", p);
    notify(api, config.admin_chat_id, &msg).await;
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

fn check_login(config: &CookieRefresherConfig) -> Result<bool, String> {
    let p = &config.profile_name;
    let sqlite = std::path::Path::new(&config.profile_path).join("cookies.sqlite");
    let exists = sqlite.exists() && sqlite.metadata().map(|m| m.len() > 0).unwrap_or(false);
    if exists {
        println!("[cookie_refresh profile={p} event=login_check] result=ok cookies.sqlite found size={}",
            sqlite.metadata().map(|m| m.len()).unwrap_or(0));
    } else {
        println!("[cookie_refresh profile={p} event=login_check] result=failed err=cookies.sqlite missing or empty path={}", sqlite.display());
    }
    Ok(exists)
}

async fn kill_existing_firefox(profile_path: &str, profile_name: &str) {
    let p = profile_name;
    println!("[cookie_refresh profile={p} event=kill_existing] killing any firefox with profile_path={profile_path}");
    let _ = Command::new("pkill")
        .arg("-f")
        .arg(format!("firefox.*{}", profile_path))
        .output()
        .await;
    sleep(Duration::from_secs(2)).await;
    println!("[cookie_refresh profile={p} event=kill_existing] done");
}

async fn open_firefox(profile_path: &str, profile_name: &str, links: &[String]) -> Result<u32, String> {
    if links.is_empty() {
        return Err("no links to open".to_string());
    }

    let p = profile_name;
    let child = Command::new("sudo")
        .arg("-u")
        .arg("mahdi")
        .arg("firefox")
        .arg("--profile")
        .arg(profile_path)
        .arg(&links[0])
        .env("DISPLAY", ":10")
        .env("XDG_RUNTIME_DIR", "/run/user/1002")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn firefox: {e}"))?;

    let pid = child.id().ok_or("firefox has no pid")?;
    println!("[cookie_refresh profile={p} event=firefox_open] pid={pid} url={}", links[0]);

    for (i, url) in links[1..].iter().enumerate() {
        sleep(Duration::from_secs(1)).await;
        let _ = Command::new("sudo")
            .arg("-u")
            .arg("mahdi")
            .arg("firefox")
            .arg("--profile")
            .arg(profile_path)
            .arg("--new-tab")
            .arg(url)
            .env("DISPLAY", ":10")
            .env("XDG_RUNTIME_DIR", "/run/user/1002")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        println!("[cookie_refresh profile={p} event=firefox_tab] tab={} url={url}", i + 2);
    }

    Ok(pid)
}

async fn wait_or_crash(pid: u32, duration_secs: u64, profile_name: &str) -> bool {
    let p = profile_name;
    let deadline = sleep(Duration::from_secs(duration_secs));
    tokio::pin!(deadline);

    let mut elapsed_checks: u64 = 0;
    loop {
        tokio::select! {
            _ = &mut deadline => {
                println!("[cookie_refresh profile={p} event=firefox_wait] elapsed={duration_secs}s pid={pid} alive=true timeout=true");
                return false;
            }
            _ = sleep(Duration::from_secs(5)) => {
                elapsed_checks += 5;
                if !is_running(pid) {
                    println!("[cookie_refresh profile={p} event=firefox_crashed] elapsed={elapsed_checks}s pid={pid}");
                    return true;
                }
                println!("[cookie_refresh profile={p} event=firefox_wait] elapsed={elapsed_checks}s pid={pid} alive=true");
            }
        }
    }
}

fn is_running(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

async fn kill_firefox(pid: u32, profile_name: &str) {
    let p = profile_name;
    println!("[cookie_refresh profile={p} event=firefox_kill_term] pid={pid}");
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .output()
        .await;
    sleep(Duration::from_secs(3)).await;
    if is_running(pid) {
        println!("[cookie_refresh profile={p} event=firefox_kill_force] pid={pid}");
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(pid.to_string())
            .output()
            .await;
    }
}

fn refresh_cache(source_dir: &str, cache_dir: &str, profile_name: &str) {
    let p = profile_name;
    println!("[cookie_refresh profile={p} event=cache_start] src={source_dir} dst={cache_dir}");
    for name in ["cookies.sqlite", "cookies.sqlite-wal", "cookies.sqlite-shm"] {
        let src = std::path::Path::new(source_dir).join(name);
        if !src.exists() {
            println!("[cookie_refresh profile={p} event=cache_copy] file={name} ok=false err=src_not_found");
            continue;
        }
        let dst = std::path::Path::new(cache_dir).join(name);
        match std::fs::copy(&src, &dst) {
            Ok(_) => println!("[cookie_refresh profile={p} event=cache_copy] file={name} ok=true"),
            Err(e) => println!("[cookie_refresh profile={p} event=cache_copy] file={name} ok=false err={e}"),
        }
    }
}

async fn notify(api: &Bot, chat_id: i64, text: &str) {
    if chat_id == 0 {
        return;
    }
    if let Err(e) = send_text(api, chat_id, text).await {
        eprintln!("[cookie_refresh event=notify_failed] chat_id={chat_id} err={e}");
    }
}
