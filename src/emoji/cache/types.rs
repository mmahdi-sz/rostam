use std::collections::HashMap;

#[derive(Clone)]
pub struct EmojiEntry {
    pub custom_emoji_id: String,
    pub fallback: String,
}

#[derive(Clone, Default)]
pub struct EmojiCache {
    pub(super) by_key: HashMap<String, Vec<EmojiEntry>>,
}
