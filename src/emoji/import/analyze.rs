use tokio_postgres::Client;

use super::types::{ImportAnalysis, ParsedSql};

pub async fn analyze(parsed: &ParsedSql, client: &Client, owner: i64) -> ImportAnalysis {
    let db_packs = client
        .query_one("SELECT COUNT(*) FROM emoji_packs WHERE owner_user_id = $1", &[&owner])
        .await.map(|r| r.get::<_, i64>(0) as usize).unwrap_or(0);

    let db_items = client
        .query_one("SELECT COUNT(*) FROM emoji_items WHERE owner_user_id = $1", &[&owner])
        .await.map(|r| r.get::<_, i64>(0) as usize).unwrap_or(0);

    let file_ids: Vec<String> = parsed.items.iter().map(|i| i.custom_emoji_id.clone()).collect();
    let duplicate_items = if file_ids.is_empty() || db_items == 0 {
        0
    } else {
        client.query_one(
            "SELECT COUNT(*) FROM emoji_items WHERE owner_user_id = $1 AND custom_emoji_id = ANY($2)",
            &[&owner, &file_ids],
        ).await.map(|r| r.get::<_, i64>(0) as usize).unwrap_or(0)
    };

    ImportAnalysis {
        file_packs: parsed.packs.len(),
        file_items: parsed.items.len(),
        db_packs,
        db_items,
        duplicate_items,
        db_empty: db_packs == 0,
    }
}
