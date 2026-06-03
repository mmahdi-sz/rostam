use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;

mod types;
mod render;
mod loader;

pub use types::{EmojiCache, EmojiEntry};
pub use render::{LookupOutcome, RenderLookup};
pub use loader::load_from_db;

pub static CACHE: OnceLock<Arc<RwLock<EmojiCache>>> = OnceLock::new();

pub fn global() -> Option<Arc<RwLock<EmojiCache>>> {
    CACHE.get().cloned()
}

static NEXT_TRACE_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_trace_id() -> u64 {
    NEXT_TRACE_ID.fetch_add(1, Ordering::Relaxed)
}

/// Returns a short, log-safe preview of `text`, truncated at character
/// boundaries to roughly `max_chars` (adds an ellipsis marker if cut).
pub fn preview(text: &str, max_chars: usize) -> String {
    let mut taken = 0usize;
    let mut out = String::new();
    for c in text.chars() {
        if taken >= max_chars { out.push('…'); break; }
        out.push(c);
        taken += 1;
    }
    out
}

/// Summarises a slice of `RenderLookup` records as a single compact log
/// fragment, e.g. `cache_hit=2 raw_id=1 not_found=1 unclosed=0`.
pub fn summarise_lookups(lookups: &[RenderLookup]) -> String {
    let mut hit = 0usize;
    let mut raw = 0usize;
    let mut miss = 0usize;
    let mut unclosed = 0usize;
    for l in lookups {
        match l.outcome {
            LookupOutcome::CacheHit { .. } => hit += 1,
            LookupOutcome::RawId => raw += 1,
            LookupOutcome::NotFound => miss += 1,
            LookupOutcome::UnclosedBrace => unclosed += 1,
        }
    }
    format!("cache_hit={hit} raw_id={raw} not_found={miss} unclosed={unclosed}")
}
