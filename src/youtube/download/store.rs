use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use super::super::trace::log_trace;
use super::types::YoutubeRequest;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static REQUESTS: OnceLock<Mutex<HashMap<u64, YoutubeRequest>>> = OnceLock::new();

fn store() -> &'static Mutex<HashMap<u64, YoutubeRequest>> {
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
    store().lock().unwrap().insert(id, req);
    id
}

pub fn get_request(id: u64) -> Option<YoutubeRequest> {
    store().lock().unwrap().get(&id).cloned()
}

pub fn take_request(id: u64) -> Option<YoutubeRequest> {
    store().lock().unwrap().remove(&id)
}
