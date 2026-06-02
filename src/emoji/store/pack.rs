use tokio_postgres::Client;

use super::{row_to_pack, EmojiPack};

pub async fn create_pack(client: &Client, owner: i64, name: &str) -> Result<EmojiPack, tokio_postgres::Error> {
    let has_any = client
        .query_one("SELECT EXISTS(SELECT 1 FROM emoji_packs WHERE owner_user_id = $1)", &[&owner])
        .await?.get::<_, bool>(0);
    let is_default = !has_any;
    let row = client
        .query_one(
            "INSERT INTO emoji_packs (owner_user_id, name, is_default)
             VALUES ($1, $2, $3)
             RETURNING id, owner_user_id, name, alias, is_default",
            &[&owner, &name, &is_default],
        )
        .await?;
    Ok(EmojiPack { id: row.get(0), owner_user_id: row.get(1), name: row.get(2), alias: row.get(3), is_default: row.get(4), item_count: 0 })
}

pub async fn find_pack_by_name(client: &Client, owner: i64, name: &str) -> Result<Option<EmojiPack>, tokio_postgres::Error> {
    let row = client.query_opt(
        "SELECT p.id, p.owner_user_id, p.name, p.alias, p.is_default,
                COALESCE((SELECT COUNT(*) FROM emoji_items i WHERE i.pack_id = p.id), 0)
         FROM emoji_packs p WHERE p.owner_user_id = $1 AND p.name = $2",
        &[&owner, &name],
    ).await?;
    Ok(row.map(row_to_pack))
}

pub async fn list_packs(client: &Client, owner: i64) -> Result<Vec<EmojiPack>, tokio_postgres::Error> {
    let rows = client.query(
        "SELECT p.id, p.owner_user_id, p.name, p.alias, p.is_default,
                COALESCE((SELECT COUNT(*) FROM emoji_items i WHERE i.pack_id = p.id), 0)
         FROM emoji_packs p WHERE p.owner_user_id = $1 ORDER BY p.id ASC",
        &[&owner],
    ).await?;
    Ok(rows.into_iter().map(row_to_pack).collect())
}

pub async fn get_default_pack(client: &Client, owner: i64) -> Result<Option<EmojiPack>, tokio_postgres::Error> {
    let row = client.query_opt(
        "SELECT p.id, p.owner_user_id, p.name, p.alias, p.is_default,
                COALESCE((SELECT COUNT(*) FROM emoji_items i WHERE i.pack_id = p.id), 0)
         FROM emoji_packs p WHERE p.owner_user_id = $1 AND p.is_default = TRUE",
        &[&owner],
    ).await?;
    Ok(row.map(row_to_pack))
}

pub async fn set_default_pack(client: &Client, owner: i64, pack_id: i32) -> Result<(), tokio_postgres::Error> {
    client.execute("UPDATE emoji_packs SET is_default = FALSE WHERE owner_user_id = $1", &[&owner]).await?;
    client.execute("UPDATE emoji_packs SET is_default = TRUE WHERE id = $1 AND owner_user_id = $2", &[&pack_id, &owner]).await?;
    Ok(())
}

pub async fn set_pack_alias(client: &Client, owner: i64, pack_id: i32, alias: Option<&str>) -> Result<(), tokio_postgres::Error> {
    client.execute(
        "UPDATE emoji_packs SET alias = $3 WHERE id = $1 AND owner_user_id = $2",
        &[&pack_id, &owner, &alias],
    ).await?;
    Ok(())
}

pub async fn delete_pack(client: &Client, owner: i64, pack_id: i32) -> Result<(), tokio_postgres::Error> {
    let was_default = client
        .query_opt("SELECT is_default FROM emoji_packs WHERE id = $1 AND owner_user_id = $2", &[&pack_id, &owner])
        .await?.map(|r| r.get::<_, bool>(0)).unwrap_or(false);

    client.execute("DELETE FROM emoji_packs WHERE id = $1 AND owner_user_id = $2", &[&pack_id, &owner]).await?;

    if was_default {
        if let Some(next) = client.query_opt(
            "SELECT id FROM emoji_packs WHERE owner_user_id = $1 ORDER BY id ASC LIMIT 1",
            &[&owner],
        ).await? {
            let next_id: i32 = next.get(0);
            set_default_pack(client, owner, next_id).await?;
        }
    }

    let no_packs = client
        .query_one("SELECT COUNT(*) FROM emoji_packs WHERE owner_user_id = $1", &[&owner])
        .await.map(|r| r.get::<_, i64>(0) == 0).unwrap_or(false);

    if no_packs {
        let _ = client.execute("ALTER SEQUENCE emoji_packs_id_seq RESTART WITH 1", &[]).await;
        let _ = client.execute("ALTER SEQUENCE emoji_items_id_seq RESTART WITH 1", &[]).await;
    }

    Ok(())
}
