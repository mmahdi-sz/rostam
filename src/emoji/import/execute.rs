use std::collections::{HashMap, HashSet};

use tokio_postgres::Client;

use crate::emoji::smart_name::base_smart_name;

use super::types::{ImportResult, ParsedSql};

pub async fn execute_replace(parsed: &ParsedSql, client: &Client, owner: i64) -> Result<ImportResult, tokio_postgres::Error> {
    client.execute("DELETE FROM emoji_packs WHERE owner_user_id = $1", &[&owner]).await?;
    insert_parsed(parsed, client, owner, false).await
}

pub async fn execute_merge(parsed: &ParsedSql, client: &Client, owner: i64, skip_duplicates: bool) -> Result<ImportResult, tokio_postgres::Error> {
    insert_parsed(parsed, client, owner, skip_duplicates).await
}

async fn insert_parsed(parsed: &ParsedSql, client: &Client, owner: i64, skip_duplicates: bool) -> Result<ImportResult, tokio_postgres::Error> {
    let mut pack_id_map: HashMap<i32, i32> = HashMap::new();
    let mut packs_added = 0usize;

    for pack in &parsed.packs {
        let existing = client.query_opt(
            "SELECT id FROM emoji_packs WHERE owner_user_id = $1 AND name = $2",
            &[&owner, &pack.name],
        ).await?;

        let new_id = if let Some(row) = existing {
            row.get::<_, i32>(0)
        } else {
            let row = client.query_one(
                "INSERT INTO emoji_packs (owner_user_id, name, alias, is_default) VALUES ($1, $2, $3, $4) RETURNING id",
                &[&owner, &pack.name, &pack.alias, &pack.is_default],
            ).await?;
            packs_added += 1;
            row.get::<_, i32>(0)
        };
        pack_id_map.insert(pack.old_id, new_id);
    }

    let existing_ids: HashSet<String> = if skip_duplicates {
        client.query("SELECT custom_emoji_id FROM emoji_items WHERE owner_user_id = $1", &[&owner])
            .await?.into_iter().map(|r| r.get::<_, String>(0)).collect()
    } else {
        HashSet::new()
    };

    let mut items_added = 0usize;
    let mut items_skipped = 0usize;

    for item in &parsed.items {
        let Some(&new_pack_id) = pack_id_map.get(&item.old_pack_id) else { items_skipped += 1; continue };
        if skip_duplicates && existing_ids.contains(&item.custom_emoji_id) { items_skipped += 1; continue }

        let smart = allocate_smart_name(client, owner, &item.fallback).await?;

        if let Err(e) = client.execute(
            "INSERT INTO emoji_items (pack_id, owner_user_id, custom_emoji_id, fallback, smart_name, alias, position) VALUES ($1, $2, $3, $4, $5, $6, $7)",
            &[&new_pack_id, &owner, &item.custom_emoji_id, &item.fallback, &smart, &item.alias, &item.position],
        ).await {
            eprintln!("insert item failed: {e}");
            items_skipped += 1;
            continue;
        }
        items_added += 1;
    }

    Ok(ImportResult { packs_added, items_added, items_skipped })
}

async fn allocate_smart_name(client: &Client, owner: i64, fallback: &str) -> Result<String, tokio_postgres::Error> {
    let base = base_smart_name(fallback);
    let row = client.query_one(
        "SELECT COALESCE(MAX(CAST(NULLIF(REGEXP_REPLACE(smart_name, '^' || $2 || '(\\d+)$', '\\1'), smart_name) AS INT)), 0)
         FROM emoji_items WHERE owner_user_id = $1 AND smart_name ~ ('^' || $2 || '\\d+$')",
        &[&owner, &base],
    ).await?;
    let max_n: i32 = row.get(0);
    Ok(format!("{}{}", base, max_n + 1))
}
