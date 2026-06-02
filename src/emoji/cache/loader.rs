use std::collections::HashMap;

use tokio_postgres::Client;

use super::types::{EmojiCache, EmojiEntry};

pub async fn load_from_db(client: &Client, admin_id: i64) -> EmojiCache {
    let rows = match client
        .query(
            "SELECT ei.id, ei.custom_emoji_id, ei.fallback, ei.smart_name, ei.alias, \
                    ep.id, ep.name, ep.alias \
             FROM emoji_items ei \
             JOIN emoji_packs ep ON ei.pack_id = ep.id \
             WHERE ei.owner_user_id = $1",
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
        let item_id: i32 = row.get(0);
        let custom_emoji_id: String = row.get(1);
        let fallback: String = row.get(2);
        let smart_name: String = row.get(3);
        let alias: Option<String> = row.get(4);
        let pack_id: i32 = row.get(5);
        let pack_name: String = row.get(6);
        let pack_alias: Option<String> = row.get(7);

        let entry = EmojiEntry { custom_emoji_id: custom_emoji_id.clone(), fallback };

        let prefix = smart_name.trim_end_matches(|c: char| c.is_ascii_digit());
        let has_prefix = !prefix.is_empty() && prefix != smart_name;
        let alias_str: Option<&str> = alias.as_deref().filter(|a| !a.is_empty());
        let pack_alias_str: Option<&str> = pack_alias.as_deref().filter(|a| !a.is_empty());

        // ── global keys ──────────────────────────────────────────────
        push(&mut by_key, &smart_name, entry.clone());          // {fire1}
        if has_prefix { push(&mut by_key, prefix, entry.clone()); } // {fire}
        if let Some(a) = alias_str {
            push(&mut by_key, a, entry.clone());                // {boss}
        }
        push(&mut by_key, &custom_emoji_id, entry.clone());     // {5188481279963715781}
        push(&mut by_key, &item_id.to_string(), entry.clone()); // {43}

        // ── pack-scoped keys ─────────────────────────────────────────
        // pack identifiers: name, numeric id, and alias (if set)
        let mut pack_idents: Vec<String> = vec![pack_name.clone(), pack_id.to_string()];
        if let Some(pa) = pack_alias_str {
            pack_idents.push(pa.to_string());
        }

        for pi in &pack_idents {
            push(&mut by_key, &format!("{pi}:{smart_name}"), entry.clone()); // {terraria:fire1}
            if has_prefix {
                push(&mut by_key, &format!("{pi}:{prefix}"), entry.clone()); // {terraria:fire}
            }
            if let Some(a) = alias_str {
                push(&mut by_key, &format!("{pi}:{a}"), entry.clone());      // {terraria:boss}
            }
            push(&mut by_key, &format!("{pi}:{item_id}"), entry.clone());    // {2:43}
        }
    }

    EmojiCache { by_key }
}

fn push(map: &mut HashMap<String, Vec<EmojiEntry>>, key: &str, entry: EmojiEntry) {
    map.entry(key.to_string()).or_default().push(entry);
}
