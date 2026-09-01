use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use super::super::trace::log_trace;
use super::types::YoutubeRequest;

struct StoredRequest {
    req: YoutubeRequest,
    created_at: std::time::Instant,
}

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static REQUESTS: OnceLock<Mutex<HashMap<u64, StoredRequest>>> = OnceLock::new();

fn store() -> &'static Mutex<HashMap<u64, StoredRequest>> {
    REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn store_request(req: YoutubeRequest) -> u64 {
    let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    log_trace(
        req.trace_id,
        "request_stored",
        &format!(
            "request_id={id} chat_id={} user_id={:?} formats={}",
            req.chat_id,
            req.user_id,
            req.formats.len()
        ),
    );
    let now = std::time::Instant::now();
    let mut map = crate::sync_util::lock_or_recover(store());
    // Auto-sweep items older than 2 hours to prevent memory leaks from abandoned menus
    map.retain(|_, item| {
        now.duration_since(item.created_at) < std::time::Duration::from_secs(7200)
    });
    map.insert(
        id,
        StoredRequest {
            req,
            created_at: now,
        },
    );
    id
}

pub fn get_request(id: u64) -> Option<YoutubeRequest> {
    let map = crate::sync_util::lock_or_recover(store());
    let item = map.get(&id)?;
    if item.created_at.elapsed() > std::time::Duration::from_secs(7200) {
        None
    } else {
        Some(item.req.clone())
    }
}

pub fn take_request(id: u64) -> Option<YoutubeRequest> {
    let mut map = crate::sync_util::lock_or_recover(store());
    let item = map.remove(&id)?;
    if item.created_at.elapsed() > std::time::Duration::from_secs(7200) {
        None
    } else {
        Some(item.req)
    }
}
