use tokio_postgres::Client;

use crate::emoji::smart_name::base_smart_name;

use super::{row_to_item, EmojiItem};

pub async fn list_items(client: &Client, pack_id: i32) -> Result<Vec<EmojiItem>, tokio_postgres::Error> {
    let rows = client.query(
        "SELECT id, pack_id, custom_emoji_id, fallback, smart_name, alias, position
         FROM emoji_items WHERE pack_id = $1 ORDER BY position ASC, id ASC",
        &[&pack_id],
    ).await?;
    Ok(rows.into_iter().map(row_to_item).collect())
}

pub async fn existing_custom_emoji_ids(client: &Client, owner: i64, ids: &[String]) -> Result<Vec<String>, tokio_postgres::Error> {
    if ids.is_empty() { return Ok(Vec::new()); }
    let rows = client.query(
        "SELECT custom_emoji_id FROM emoji_items WHERE owner_user_id = $1 AND custom_emoji_id = ANY($2)",
        &[&owner, &ids],
    ).await?;
    Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
}

pub async fn allocate_smart_name(client: &Client, owner: i64, fallback: &str) -> Result<String, tokio_postgres::Error> {
    let base = base_smart_name(fallback);
    let row = client.query_one(
        "SELECT COALESCE(MAX(
            CAST(NULLIF(REGEXP_REPLACE(smart_name, '^' || $2 || '(\\d+)$', '\\1'), smart_name) AS INT)
         ), 0)
         FROM emoji_items WHERE owner_user_id = $1 AND smart_name ~ ('^' || $2 || '\\d+$')",
        &[&owner, &base],
    ).await?;
    let next: i32 = row.get(0);
    Ok(format!("{}{}", base, next + 1))
}

pub async fn add_item(
    client: &Client, owner: i64, pack_id: i32,
    custom_emoji_id: &str, fallback: &str, smart_name: &str,
) -> Result<EmojiItem, tokio_postgres::Error> {
    let position: i32 = client
        .query_one("SELECT COALESCE(MAX(position), 0) + 1 FROM emoji_items WHERE pack_id = $1", &[&pack_id])
        .await?.get(0);
    let row = client.query_one(
        "INSERT INTO emoji_items (pack_id, owner_user_id, custom_emoji_id, fallback, smart_name, position)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, pack_id, custom_emoji_id, fallback, smart_name, alias, position",
        &[&pack_id, &owner, &custom_emoji_id, &fallback, &smart_name, &position],
    ).await?;
    Ok(row_to_item(row))
}

pub async fn set_item_alias(client: &Client, owner: i64, selector: &str, alias: Option<&str>) -> Result<bool, tokio_postgres::Error> {
    let target_id: Option<i32> = if let Ok(id) = selector.parse::<i32>() {
        client.query_opt(
            "SELECT id FROM emoji_items WHERE id = $1 AND owner_user_id = $2",
            &[&id, &owner],
        ).await?.map(|r| r.get(0))
    } else {
        client.query_opt(
            "SELECT id FROM emoji_items WHERE owner_user_id = $1 AND (smart_name = $2 OR alias = $2) LIMIT 1",
            &[&owner, &selector],
        ).await?.map(|r| r.get(0))
    };

    let Some(id) = target_id else { return Ok(false) };
    client.execute(
        "UPDATE emoji_items SET alias = $1 WHERE id = $2 AND owner_user_id = $3",
        &[&alias, &id, &owner],
    ).await?;
    Ok(true)
}
