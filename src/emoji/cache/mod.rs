use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;

mod types;
mod render;
mod loader;

pub use types::{EmojiCache, EmojiEntry};
pub use loader::load_from_db;

pub static CACHE: OnceLock<Arc<RwLock<EmojiCache>>> = OnceLock::new();

pub fn global() -> Option<Arc<RwLock<EmojiCache>>> {
    CACHE.get().cloned()
}
