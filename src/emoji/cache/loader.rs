use std::collections::HashMap;

use tokio_postgres::Client;

use super::types::{EmojiCache, EmojiEntry};

pub async fn load_from_db(client: &Client, admin_id: i64) -> EmojiCache {
    let rows = match client
        .query(
            "SELECT custom_emoji_id, fallback, smart_name, alias \
             FROM emoji_items WHERE owner_user_id = $1",
            &[&admin_id],
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("emoji cache load failed: {e}");
            return EmojiCache::default();
        }
    };

    let mut by_key: HashMap<String, Vec<EmojiEntry>> = HashMap::new();

    for row in rows {
        let custom_emoji_id: String = row.get(0);
        let fallback: String = row.get(1);
        let smart_name: String = row.get(2);
        let alias: Option<String> = row.get(3);

        let entry = EmojiEntry { custom_emoji_id, fallback };

        by_key.entry(smart_name.clone()).or_default().push(entry.clone());

        let prefix = smart_name.trim_end_matches(|c: char| c.is_ascii_digit());
        if !prefix.is_empty() && prefix != smart_name {
            by_key.entry(prefix.to_string()).or_default().push(entry.clone());
        }

        if let Some(a) = alias.filter(|a| !a.is_empty()) {
            by_key.entry(a).or_default().push(entry.clone());
        }

        // also index by raw custom_emoji_id so {5188481279963715781} works directly
        by_key.entry(entry.custom_emoji_id.clone()).or_default().push(entry);
    }

    EmojiCache { by_key }
}
