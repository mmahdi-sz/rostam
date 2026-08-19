//! Spotify and SoundCloud album/playlist handler.
//!
//! Both platforms share a common queue: set link is provided, track list is fetched,
//! user selects upload mode (individual tracks / 7z archive), then `runner`
//! executes the single-track flow for each platform in a loop.

pub mod runner;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use frankenstein::client_reqwest::Bot;

use crate::database::postgresql::PostgresDatabase;
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::rank::{self, types::Rank};
use crate::spotify::client::SpotifySetItem;
use crate::spotify::extract::SpotifySetKind;

pub const CB_MS_MODE_ONE: &str = "ms:mode:one";
pub const CB_MS_MODE_ZIP: &str = "ms:mode:zip";
/// Cancel before job start (start menu)
pub const CB_MS_CANCEL: &str = "ms:cancel";
/// Cancel during download
pub const CB_MS_JOBCANCEL: &str = "ms:jobcancel";

/// Archive split size; local Bot API uploads up to 2GB.
pub const MS_SPLIT_MB: u32 = 1900;

#[derive(Debug, Clone)]
pub enum SetItems {
    /// Spotify items include embed page metadata.
    Spotify(Vec<SpotifySetItem>),
    /// SoundCloud only provides permalinks; metadata fetched upon download.
    Soundcloud(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct PendingSet {
    /// Origin platform log/stats domain: `sp` or `sc`
    pub domain: &'static str,
    pub title: String,
    pub items: SetItems,
    /// Actual track count of the set, before applying rank cap.
    pub total_before_cap: usize,
}

impl PendingSet {
    pub fn len(&self) -> usize {
        match &self.items {
            SetItems::Spotify(v) => v.len(),
            SetItems::Soundcloud(v) => v.len(),
        }
    }

    /// Satisfies `len_without_is_empty` lint
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

struct StoredPendingSet {
    set: PendingSet,
    created_at: std::time::Instant,
}

/// Sets fetched and awaiting upload mode selection.
///
/// Keyed by `(user_id, offer_message_id)` so old messages do not execute newer link sets.
static PENDING_SETS: LazyLock<Mutex<HashMap<(i64, i32), StoredPendingSet>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Active job cancellation flag for progress message cancel button.
static ACTIVE_MS_JOBS: LazyLock<Mutex<HashMap<i64, Arc<AtomicBool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Stores new set and purges old offers for the same user.
pub fn put_pending(user_id: i64, message_id: i32, set: PendingSet) {
    if let Ok(mut m) = PENDING_SETS.lock() {
        let now = std::time::Instant::now();
        // Purge offers for this user or older than 1 hour (3600s)
        m.retain(|(u, mid), item| (*u != user_id || *mid == message_id) && now.duration_since(item.created_at) < std::time::Duration::from_secs(3600));
        m.insert((user_id, message_id), StoredPendingSet { set, created_at: now });
    }
}

pub fn take_pending(user_id: i64, message_id: i32) -> Option<PendingSet> {
    let mut m = PENDING_SETS.lock().ok()?;
    let item = m.remove(&(user_id, message_id))?;
    if item.created_at.elapsed() > std::time::Duration::from_secs(3600) {
        None
    } else {
        Some(item.set)
    }
}

pub fn register_cancel(user_id: i64) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut jobs) = ACTIVE_MS_JOBS.lock() {
        jobs.insert(user_id, flag.clone());
    }
    flag
}

/// Removes from registry on any exit path.
pub struct MsUnregisterGuard(pub i64);

impl Drop for MsUnregisterGuard {
    fn drop(&mut self) {
        if let Ok(mut jobs) = ACTIVE_MS_JOBS.lock() {
            jobs.remove(&self.0);
        }
    }
}

/// Returns `true` if job was active for user and cancel flag was set.
pub fn cancel_job(user_id: i64) -> bool {
    let Ok(jobs) = ACTIVE_MS_JOBS.lock() else {
        return false;
    };
    match jobs.get(&user_id) {
        Some(flag) => {
            flag.store(true, Ordering::SeqCst);
            true
        }
        None => false,
    }
}

#[derive(Debug, Clone)]
pub enum SetSource {
    Spotify(SpotifySetKind, String),
    Soundcloud(String),
}

/// User rank; defaults to Dalavar without DB.
pub async fn user_rank(database: &Option<PostgresDatabase>, user_id: i64) -> Rank {
    match database {
        Some(db) => {
            if let Ok(client) = db.get().await {
                rank::effective_rank(&client, user_id).await
            } else {
                Rank::Dalavar
            }
        }
        None => Rank::Dalavar,
    }
}

/// Detects set link in text.
///
/// Checks Spotify first, as `extract_soundcloud_url` captures any two-part path.
pub fn detect_set(text: &str) -> Option<SetSource> {
    if let Some((kind, id)) = crate::spotify::extract::extract_spotify_set(text) {
        return Some(SetSource::Spotify(kind, id));
    }
    crate::soundcloud::extract::extract_soundcloud_set_url(text).map(SetSource::Soundcloud)
}

/// Spawns task and returns `true` if text is an album/playlist link.
///
/// Must be invoked before single track detection.
pub fn try_route_set(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    database: &Option<PostgresDatabase>,
    text: &str,
) -> bool {
    let Some(source) = detect_set(text) else {
        return false;
    };
    let api = api.clone();
    let database = database.clone();
    let trace_id = crate::log::next_trace_id();
    crate::app::spawn_user_task(async move {
        if let Err(e) = offer_set(&api, chat_id, user_id, trace_id, source, &database).await {
            crate::stats::record_error_global("musicset", format!("offer_set: {e}")).await;
        }
    });
    true
}

/// Handles upload mode callback selection.
pub async fn handle_mode_callback(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    message_id: i32,
    zip_mode: bool,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = crate::log::next_trace_id();
    let Some(pending) = take_pending(user_id, message_id) else {
        log_ev!("ms", trace_id, "mode_pick", "=>" => "expired");
        edit_status(api, chat_id, message_id, &t("musicset.expired"), None).await;
        return;
    };

    if zip_mode && !user_rank(database, user_id).await.can_music_set_archive() {
        // Sepahbod limited to individual tracks; retain pending set.
        log_ev!("ms", trace_id, "mode_pick", "=>" => "blocked", "mode" => "zip");
        put_pending(user_id, message_id, pending);
        rank::paywall::block_limit(api, chat_id, &t("musicset.zip_limit"), Rank::Esfandyar).await;
        return;
    }

    log_ev!("ms", trace_id, "mode_pick", "=>" => "ok", "mode" => if zip_mode { "zip" } else { "one" }, "tracks" => pending.len());
    edit_status(api, chat_id, message_id, &t("musicset.starting"), None).await;

    let api = api.clone();
    let database = database.clone();
    crate::app::spawn_user_task(async move {
        runner::run_set_job(
            api, chat_id, user_id, trace_id, pending, zip_mode, message_id, database,
        )
        .await;
    });
}

pub fn mode_keyboard() -> frankenstein::types::InlineKeyboardMarkup {
    use crate::emoji::panel::{btn_icon_danger, btn_icon_primary, btn_icon_success};
    frankenstein::types::InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                btn_icon_success(&t("musicset.btn_one"), CB_MS_MODE_ONE, "music_note"),
                btn_icon_primary(&t("musicset.btn_zip"), CB_MS_MODE_ZIP, "7zip_logo"),
            ],
            vec![btn_icon_danger(
                &t("musicset.btn_cancel"),
                CB_MS_CANCEL,
                "cancel",
            )],
        ])
        .build()
}

pub fn job_cancel_keyboard() -> frankenstein::types::InlineKeyboardMarkup {
    frankenstein::types::InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![crate::emoji::panel::btn_icon_danger(
            &t("musicset.btn_cancel"),
            CB_MS_JOBCANCEL,
            "cancel",
        )]])
        .build()
}

/// Fetches track list and prompts for upload mode choice.
pub async fn offer_set(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    trace_id: u64,
    source: SetSource,
    database: &Option<PostgresDatabase>,
) -> anyhow::Result<()> {
    log_actor_id!("ms", trace_id, user_id, "src" => match &source {
        SetSource::Spotify(k, id) => format!("sp:{}:{id}", k.as_str()),
        SetSource::Soundcloud(u) => format!("sc:{u}"),
    });

    let rank_now = user_rank(database, user_id).await;
    let limit = rank_now.music_set_limit();

    log_ev!("ms", trace_id, "paywall_check", "rank" => rank_now.as_str(), "limit" => format!("{limit:?}"));
    if limit == Some(0) {
        log_ev!("ms", trace_id, "paywall_check", "=>" => "blocked");
        // Sepahbod already gets 20 tracks — naming Esfandyar here contradicted the
        // rank guide and oversold the cheapest rank that unlocks the feature.
        rank::paywall::block_feature(api, chat_id, &t("musicset.feature_name"), Rank::Sepahbod)
            .await;
        return Ok(());
    }

    let status_msg_id = send_status(api, chat_id, &t("musicset.fetching_list")).await;

    log_ev!("ms", trace_id, "fetch_list_enter", "=>" => "start");
    let pending = match fetch_set(trace_id, &source).await {
        Ok(p) => p,
        Err(e) => {
            log_ev!("ms", trace_id, "fetch_list_enter", "=>" => "fail", "err" => e.to_string());
            crate::stats::record_error_global("musicset", format!("fetch_list: {e}")).await;
            // Private/deleted set has its own error message.
            let msg = e.to_string().to_lowercase();
            let key = if msg.contains("404") || msg.contains("private") {
                "musicset.list_private"
            } else {
                "musicset.list_failed"
            };
            edit_status(api, chat_id, status_msg_id, &t(key), None).await;
            // Re-arm start menu prompt.
            let _ = crate::bot::send_start_menu(api, chat_id).await;
            return Ok(());
        }
    };

    let total = pending.len();
    let capped = match limit {
        Some(max) if total > max as usize => {
            let max = max as usize;
            let items = match pending.items {
                SetItems::Spotify(mut v) => {
                    v.truncate(max);
                    SetItems::Spotify(v)
                }
                SetItems::Soundcloud(mut v) => {
                    v.truncate(max);
                    SetItems::Soundcloud(v)
                }
            };
            PendingSet { items, ..pending }
        }
        _ => pending,
    };

    log_ev!("ms", trace_id, "fetch_list_enter", "=>" => "ok", "tracks" => total, "queued" => capped.len());

    let mut text = tf(
        "musicset.ask_mode",
        &[
            ("title", &md_escape(&capped.title)),
            ("count", &capped.len().to_string()),
        ],
    );
    if capped.len() < capped.total_before_cap {
        text.push('\n');
        text.push_str(&tf(
            "musicset.capped_notice",
            &[
                ("shown", &capped.len().to_string()),
                ("total", &capped.total_before_cap.to_string()),
            ],
        ));
    }

    put_pending(user_id, status_msg_id, capped);
    edit_status(api, chat_id, status_msg_id, &text, Some(mode_keyboard())).await;
    Ok(())
}

pub async fn fetch_set(trace_id: u64, source: &SetSource) -> anyhow::Result<PendingSet> {
    match source {
        SetSource::Spotify(kind, set_id) => {
            let set = crate::spotify::client::fetch_spotify_set(*kind, set_id).await?;
            let total = set.items.len();
            Ok(PendingSet {
                domain: "sp",
                title: set.title,
                items: SetItems::Spotify(set.items),
                total_before_cap: total,
            })
        }
        SetSource::Soundcloud(url) => {
            let set = crate::soundcloud::fetch::fetch_soundcloud_set(trace_id, url).await?;
            let total = set.track_urls.len();
            Ok(PendingSet {
                domain: "sc",
                title: set.title,
                items: SetItems::Soundcloud(set.track_urls),
                total_before_cap: total,
            })
        }
    }
}

pub(crate) async fn send_status(api: &Bot, chat_id: i64, text_md: &str) -> i32 {
    use frankenstein::{AsyncTelegramApi, ParseMode, methods::SendMessageParams};
    api.send_message(
        &SendMessageParams::builder()
            .chat_id(chat_id)
            .text(apply_premium_to_md(text_md))
            .parse_mode(ParseMode::MarkdownV2)
            .build(),
    )
    .await
    .map(|r| r.result.message_id)
    .unwrap_or(0)
}

pub(crate) async fn edit_status(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    text_md: &str,
    kb: Option<frankenstein::types::InlineKeyboardMarkup>,
) {
    use frankenstein::{AsyncTelegramApi, ParseMode, methods::EditMessageTextParams};
    if message_id <= 0 {
        return;
    }
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(apply_premium_to_md(text_md))
        .parse_mode(ParseMode::MarkdownV2)
        .build();
    params.reply_markup = kb;
    let _ = api.edit_message_text(&params).await;
}
