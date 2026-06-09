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
            duration_secs: 600,
            link_count: 1,
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

    // Force-enable autoplay so YouTube videos actually play (Firefox blocks
    // audible autoplay by default). Muted (volume_scale=0) so no real audio
    // sink is needed — the box has no PCM device. Without this, the video sits
    // paused and YouTube never registers an active-watch session.
    write_autoplay_prefs(&config.profile_path, p);

    kill_existing_firefox(&config.profile_path, p).await;
    open_firefox(&config.profile_path, p, &links).await?;

    let crashed = wait_or_crash(&config.profile_path, config.duration_secs, p).await;

    kill_firefox(&config.profile_path, p).await;

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
    let pattern = format!("firefox.*{}", profile_path);

    println!("[cookie_refresh profile={p} event=kill_existing] pkill -TERM profile_path={profile_path}");
    let _ = Command::new("pkill")
        .arg("-f")
        .arg(&pattern)
        .output()
        .await;
    sleep(Duration::from_secs(3)).await;

    println!("[cookie_refresh profile={p} event=kill_existing] pkill -KILL profile_path={profile_path}");
    let _ = Command::new("pkill")
        .arg("-9")
        .arg("-f")
        .arg(&pattern)
        .output()
        .await;
    sleep(Duration::from_secs(2)).await;

    // Remove Firefox lock files so the profile opens cleanly.
    for lock in [".parentlock", "lock"] {
        let path = std::path::Path::new(profile_path).join(lock);
        if path.exists() {
            match std::fs::remove_file(&path) {
                Ok(_) => println!("[cookie_refresh profile={p} event=kill_existing] removed lock file={}", path.display()),
                Err(e) => eprintln!("[cookie_refresh profile={p} event=kill_existing] failed to remove lock file={} err={e}", path.display()),
            }
        }
    }

    println!("[cookie_refresh profile={p} event=kill_existing] done");
}

async fn open_firefox(profile_path: &str, profile_name: &str, links: &[String]) -> Result<(), String> {
    if links.is_empty() {
        return Err("no links to open".to_string());
    }

    let p = profile_name;
    // Spawn firefox via sudo. We don't track its pid — we use pgrep/pkill by profile path
    // because child.id() gives the sudo wrapper pid, not the actual firefox pid.
    let mut child = Command::new("sudo")
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

    // Detach: we don't wait on this child; we track firefox by profile path via pgrep.
    drop(child);
    println!("[cookie_refresh profile={p} event=firefox_open] url={}", links[0]);

    // Single focused tab only: background tabs get media-throttled by Firefox,
    // so opening extra --new-tab links would never play. One active video is
    // enough to keep the session warm.
    Ok(())
}

/// Writes a user.js into the profile that enables muted video autoplay.
/// user.js is re-applied by Firefox on every startup, so it reliably overrides
/// whatever is in prefs.js.
fn write_autoplay_prefs(profile_path: &str, profile_name: &str) {
    let p = profile_name;
    let user_js = std::path::Path::new(profile_path).join("user.js");
    let contents = concat!(
        "// managed by cookie_refresher — enable muted autoplay for cookie warming\n",
        "user_pref(\"media.autoplay.default\", 0);\n",
        "user_pref(\"media.autoplay.blocking_policy\", 0);\n",
        "user_pref(\"media.autoplay.block-webaudio\", false);\n",
        "user_pref(\"media.volume_scale\", \"0.0\");\n",
        "user_pref(\"browser.shell.checkDefaultBrowser\", false);\n",
        "user_pref(\"datareporting.policy.dataSubmissionEnabled\", false);\n",
    );
    match std::fs::write(&user_js, contents) {
        Ok(_) => println!("[cookie_refresh profile={p} event=autoplay_prefs_written] path={}", user_js.display()),
        Err(e) => eprintln!("[cookie_refresh profile={p} event=autoplay_prefs_failed] err={e}"),
    }
}

async fn wait_or_crash(profile_path: &str, duration_secs: u64, profile_name: &str) -> bool {
    let p = profile_name;
    let deadline = sleep(Duration::from_secs(duration_secs));
    tokio::pin!(deadline);

    let mut elapsed_checks: u64 = 0;
    loop {
        tokio::select! {
            _ = &mut deadline => {
                println!("[cookie_refresh profile={p} event=firefox_timeout] elapsed={duration_secs}s");
                return false;
            }
            _ = sleep(Duration::from_secs(5)) => {
                elapsed_checks += 5;
                if !is_firefox_running(profile_path).await {
                    println!("[cookie_refresh profile={p} event=firefox_crashed] elapsed={elapsed_checks}s");
                    return true;
                }
                println!("[cookie_refresh profile={p} event=firefox_wait] elapsed={elapsed_checks}s alive=true");
            }
        }
    }
}

async fn is_firefox_running(profile_path: &str) -> bool {
    Command::new("pgrep")
        .args(["-f", &format!("firefox.*{}", profile_path)])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn kill_firefox(profile_path: &str, profile_name: &str) {
    let p = profile_name;
    println!("[cookie_refresh profile={p} event=firefox_kill_term]");
    let _ = Command::new("pkill")
        .args(["-TERM", "-f", &format!("firefox.*{}", profile_path)])
        .output()
        .await;
    sleep(Duration::from_secs(3)).await;
    if is_firefox_running(profile_path).await {
        println!("[cookie_refresh profile={p} event=firefox_kill_force]");
        let _ = Command::new("pkill")
            .args(["-9", "-f", &format!("firefox.*{}", profile_path)])
            .output()
            .await;
        sleep(Duration::from_secs(1)).await;
    }
    reap_orphan_crashhelpers(p).await;
}

/// Firefox spawns helper processes (crashhelper) that reparent to init (PPID=1)
/// once their parent firefox dies. They don't carry the profile path in argv, so
/// the profile-path pkill misses them and they accumulate. Reap any crashhelper
/// whose referenced parent firefox pid no longer exists.
async fn reap_orphan_crashhelpers(profile_name: &str) {
    let p = profile_name;
    let out = match Command::new("pgrep").args(["-af", "crashhelper"]).output().await {
        Ok(o) => o,
        Err(_) => return,
    };
    let listing = String::from_utf8_lossy(&out.stdout);
    let mut reaped = 0u32;
    for line in listing.lines() {
        // format: "<pid> /usr/lib/firefox-esr/crashhelper <parent_pid> ..."
        let mut parts = line.split_whitespace();
        let Some(pid) = parts.next() else { continue; };
        // skip the binary path token, then read the parent firefox pid argument
        let _bin = parts.next();
        let Some(parent_pid) = parts.next() else { continue; };
        let parent_alive = std::path::Path::new(&format!("/proc/{parent_pid}")).exists();
        if !parent_alive {
            let _ = Command::new("kill").args(["-9", pid]).output().await;
            reaped += 1;
        }
    }
    if reaped > 0 {
        println!("[cookie_refresh profile={p} event=crashhelper_reaped] count={reaped}");
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
