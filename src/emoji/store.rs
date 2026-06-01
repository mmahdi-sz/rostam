use tokio_postgres::Client;

use super::smart_name::base_smart_name;

#[derive(Debug, Clone)]
pub struct EmojiPack {
    pub id: i32,
    pub owner_user_id: i64,
    pub name: String,
    pub alias: Option<String>,
    pub is_default: bool,
    pub item_count: i64,
}

#[derive(Debug, Clone)]
pub struct EmojiItem {
    pub id: i32,
    pub pack_id: i32,
    pub custom_emoji_id: String,
    pub fallback: String,
    pub smart_name: String,
    pub alias: Option<String>,
    pub position: i32,
}

pub async fn create_pack(
    client: &Client,
    owner: i64,
    name: &str,
) -> Result<EmojiPack, tokio_postgres::Error> {
    let has_any = client
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM emoji_packs WHERE owner_user_id = $1)",
            &[&owner],
        )
        .await?
        .get::<_, bool>(0);
    let is_default = !has_any;

    let row = client
        .query_one(
            "INSERT INTO emoji_packs (owner_user_id, name, is_default)
             VALUES ($1, $2, $3)
             RETURNING id, owner_user_id, name, alias, is_default",
            &[&owner, &name, &is_default],
        )
        .await?;

    Ok(EmojiPack {
        id: row.get(0),
        owner_user_id: row.get(1),
        name: row.get(2),
        alias: row.get(3),
        is_default: row.get(4),
        item_count: 0,
    })
}

pub async fn find_pack_by_name(
    client: &Client,
    owner: i64,
    name: &str,
) -> Result<Option<EmojiPack>, tokio_postgres::Error> {
    let row = client
        .query_opt(
            "SELECT p.id, p.owner_user_id, p.name, p.alias, p.is_default,
                    COALESCE((SELECT COUNT(*) FROM emoji_items i WHERE i.pack_id = p.id), 0)
             FROM emoji_packs p
             WHERE p.owner_user_id = $1 AND p.name = $2",
            &[&owner, &name],
        )
        .await?;
    Ok(row.map(row_to_pack))
}

pub async fn list_packs(
    client: &Client,
    owner: i64,
) -> Result<Vec<EmojiPack>, tokio_postgres::Error> {
    let rows = client
        .query(
            "SELECT p.id, p.owner_user_id, p.name, p.alias, p.is_default,
                    COALESCE((SELECT COUNT(*) FROM emoji_items i WHERE i.pack_id = p.id), 0)
             FROM emoji_packs p
             WHERE p.owner_user_id = $1
             ORDER BY p.id ASC",
            &[&owner],
        )
        .await?;
    Ok(rows.into_iter().map(row_to_pack).collect())
}

pub async fn get_default_pack(
    client: &Client,
    owner: i64,
) -> Result<Option<EmojiPack>, tokio_postgres::Error> {
    let row = client
        .query_opt(
            "SELECT p.id, p.owner_user_id, p.name, p.alias, p.is_default,
                    COALESCE((SELECT COUNT(*) FROM emoji_items i WHERE i.pack_id = p.id), 0)
             FROM emoji_packs p
             WHERE p.owner_user_id = $1 AND p.is_default = TRUE",
            &[&owner],
        )
        .await?;
    Ok(row.map(row_to_pack))
}

pub async fn set_default_pack(
    client: &Client,
    owner: i64,
    pack_id: i32,
) -> Result<(), tokio_postgres::Error> {
    client
        .execute(
            "UPDATE emoji_packs SET is_default = FALSE WHERE owner_user_id = $1",
            &[&owner],
        )
        .await?;
    client
        .execute(
            "UPDATE emoji_packs SET is_default = TRUE WHERE id = $1 AND owner_user_id = $2",
            &[&pack_id, &owner],
        )
        .await?;
    Ok(())
}

pub async fn set_pack_alias(
    client: &Client,
    owner: i64,
    pack_id: i32,
    alias: Option<&str>,
) -> Result<(), tokio_postgres::Error> {
    client
        .execute(
            "UPDATE emoji_packs SET alias = $3
             WHERE id = $1 AND owner_user_id = $2",
            &[&pack_id, &owner, &alias],
        )
        .await?;
    Ok(())
}

pub async fn delete_pack(
    client: &Client,
    owner: i64,
    pack_id: i32,
) -> Result<(), tokio_postgres::Error> {
    let was_default = client
        .query_opt(
            "SELECT is_default FROM emoji_packs
             WHERE id = $1 AND owner_user_id = $2",
            &[&pack_id, &owner],
        )
        .await?
        .map(|r| r.get::<_, bool>(0))
        .unwrap_or(false);

    client
        .execute(
            "DELETE FROM emoji_packs WHERE id = $1 AND owner_user_id = $2",
            &[&pack_id, &owner],
        )
        .await?;

    if was_default {
        if let Some(next) = client
            .query_opt(
                "SELECT id FROM emoji_packs WHERE owner_user_id = $1 ORDER BY id ASC LIMIT 1",
                &[&owner],
            )
            .await?
        {
            let next_id: i32 = next.get(0);
            set_default_pack(client, owner, next_id).await?;
        }
    }

    let no_packs = client
        .query_one("SELECT COUNT(*) FROM emoji_packs WHERE owner_user_id = $1", &[&owner])
        .await
        .map(|r| r.get::<_, i64>(0) == 0)
        .unwrap_or(false);

    if no_packs {
        let _ = client.execute("ALTER SEQUENCE emoji_packs_id_seq RESTART WITH 1", &[]).await;
        let _ = client.execute("ALTER SEQUENCE emoji_items_id_seq RESTART WITH 1", &[]).await;
    }

    Ok(())
}

pub async fn list_items(
    client: &Client,
    pack_id: i32,
) -> Result<Vec<EmojiItem>, tokio_postgres::Error> {
    let rows = client
        .query(
            "SELECT id, pack_id, custom_emoji_id, fallback, smart_name, alias, position
             FROM emoji_items
             WHERE pack_id = $1
             ORDER BY position ASC, id ASC",
            &[&pack_id],
        )
        .await?;
    Ok(rows.into_iter().map(row_to_item).collect())
}

pub async fn existing_custom_emoji_ids(
    client: &Client,
    owner: i64,
    ids: &[String],
) -> Result<Vec<String>, tokio_postgres::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = client
        .query(
            "SELECT custom_emoji_id FROM emoji_items
             WHERE owner_user_id = $1 AND custom_emoji_id = ANY($2)",
            &[&owner, &ids],
        )
        .await?;
    Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
}

pub async fn allocate_smart_name(
    client: &Client,
    owner: i64,
    fallback: &str,
) -> Result<String, tokio_postgres::Error> {
    let base = base_smart_name(fallback);
    let row = client
        .query_one(
            "SELECT COALESCE(MAX(
                CAST(NULLIF(REGEXP_REPLACE(smart_name, '^' || $2 || '(\\d+)$', '\\1'), smart_name) AS INT)
             ), 0)
             FROM emoji_items
             WHERE owner_user_id = $1 AND smart_name ~ ('^' || $2 || '\\d+$')",
            &[&owner, &base],
        )
        .await?;
    let next: i32 = row.get(0);
    Ok(format!("{}{}", base, next + 1))
}

pub async fn add_item(
    client: &Client,
    owner: i64,
    pack_id: i32,
    custom_emoji_id: &str,
    fallback: &str,
    smart_name: &str,
) -> Result<EmojiItem, tokio_postgres::Error> {
    let position: i32 = client
        .query_one(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM emoji_items WHERE pack_id = $1",
            &[&pack_id],
        )
        .await?
        .get(0);

    let row = client
        .query_one(
            "INSERT INTO emoji_items
                (pack_id, owner_user_id, custom_emoji_id, fallback, smart_name, position)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, pack_id, custom_emoji_id, fallback, smart_name, alias, position",
            &[
                &pack_id,
                &owner,
                &custom_emoji_id,
                &fallback,
                &smart_name,
                &position,
            ],
        )
        .await?;
    Ok(row_to_item(row))
}

pub async fn set_item_alias(
    client: &Client,
    owner: i64,
    selector: &str,
    alias: Option<&str>,
) -> Result<bool, tokio_postgres::Error> {
    let target_id: Option<i32> = if let Ok(id) = selector.parse::<i32>() {
        client
            .query_opt(
                "SELECT id FROM emoji_items WHERE id = $1 AND owner_user_id = $2",
                &[&id, &owner],
            )
            .await?
            .map(|r| r.get(0))
    } else {
        client
            .query_opt(
                "SELECT id FROM emoji_items
                 WHERE owner_user_id = $1 AND (smart_name = $2 OR alias = $2)
                 LIMIT 1",
                &[&owner, &selector],
            )
            .await?
            .map(|r| r.get(0))
    };

    let Some(id) = target_id else {
        return Ok(false);
    };

    client
        .execute(
            "UPDATE emoji_items SET alias = $1 WHERE id = $2 AND owner_user_id = $3",
            &[&alias, &id, &owner],
        )
        .await?;
    Ok(true)
}

pub async fn export_user_sql(
    client: &Client,
    owner: i64,
) -> Result<String, tokio_postgres::Error> {
    let mut out = String::new();

    out.push_str("-- emoji export\n\n");
    out.push_str("CREATE TABLE IF NOT EXISTS emoji_packs (\n");
    out.push_str("    id SERIAL PRIMARY KEY,\n");
    out.push_str("    owner_user_id BIGINT NOT NULL,\n");
    out.push_str("    name TEXT NOT NULL,\n");
    out.push_str("    alias TEXT,\n");
    out.push_str("    is_default BOOLEAN NOT NULL DEFAULT FALSE,\n");
    out.push_str("    item_count INT NOT NULL DEFAULT 0\n");
    out.push_str(");\n\n");

    out.push_str("CREATE TABLE IF NOT EXISTS emoji_items (\n");
    out.push_str("    id SERIAL PRIMARY KEY,\n");
    out.push_str("    pack_id INT NOT NULL REFERENCES emoji_packs(id) ON DELETE CASCADE,\n");
    out.push_str("    owner_user_id BIGINT NOT NULL,\n");
    out.push_str("    custom_emoji_id TEXT NOT NULL,\n");
    out.push_str("    fallback TEXT NOT NULL,\n");
    out.push_str("    smart_name TEXT NOT NULL,\n");
    out.push_str("    alias TEXT,\n");
    out.push_str("    position INT NOT NULL DEFAULT 0\n");
    out.push_str(");\n\n");

    let packs = client
        .query(
            "SELECT id, owner_user_id, name, alias, is_default FROM emoji_packs WHERE owner_user_id = $1 ORDER BY id",
            &[&owner],
        )
        .await?;

    for row in &packs {
        let id: i32 = row.get(0);
        let owner_uid: i64 = row.get(1);
        let name: String = row.get(2);
        let alias: Option<String> = row.get(3);
        let is_default: bool = row.get(4);
        let alias_sql = match &alias {
            Some(a) => format!("'{}'", a.replace('\'', "''")),
            None => "NULL".to_string(),
        };
        out.push_str(&format!(
            "INSERT INTO emoji_packs (id, owner_user_id, name, alias, is_default) VALUES ({id}, {owner_uid}, '{}', {alias_sql}, {is_default});\n",
            name.replace('\'', "''")
        ));
    }

    if !packs.is_empty() {
        out.push('\n');
    }

    let items = client
        .query(
            "SELECT id, pack_id, owner_user_id, custom_emoji_id, fallback, smart_name, alias, position FROM emoji_items WHERE owner_user_id = $1 ORDER BY id",
            &[&owner],
        )
        .await?;

    for row in &items {
        let id: i32 = row.get(0);
        let pack_id: i32 = row.get(1);
        let owner_uid: i64 = row.get(2);
        let custom_emoji_id: String = row.get(3);
        let fallback: String = row.get(4);
        let smart_name: String = row.get(5);
        let alias: Option<String> = row.get(6);
        let position: i32 = row.get(7);
        let alias_sql = match &alias {
            Some(a) => format!("'{}'", a.replace('\'', "''")),
            None => "NULL".to_string(),
        };
        out.push_str(&format!(
            "INSERT INTO emoji_items (id, pack_id, owner_user_id, custom_emoji_id, fallback, smart_name, alias, position) VALUES ({id}, {pack_id}, {owner_uid}, '{}', '{}', '{}', {alias_sql}, {position});\n",
            custom_emoji_id.replace('\'', "''"),
            fallback.replace('\'', "''"),
            smart_name.replace('\'', "''"),
        ));
    }

    Ok(out)
}

pub async fn render_template(
    client: &Client,
    owner: i64,
    template: &str,
) -> Result<String, tokio_postgres::Error> {
    let rows = client
        .query(
            "SELECT custom_emoji_id, fallback, smart_name, alias
             FROM emoji_items
             WHERE owner_user_id = $1",
            &[&owner],
        )
        .await?;

    let mut result = template.to_string();
    for row in rows {
        let custom_emoji_id: String = row.get(0);
        let fallback: String = row.get(1);
        let smart_name: String = row.get(2);
        let alias: Option<String> = row.get(3);

        let replacement = format!("![{}](tg://emoji?id={})", fallback, custom_emoji_id);
        result = result.replace(&format!("{{{}}}", smart_name), &replacement);
        if let Some(a) = alias.filter(|a| !a.is_empty()) {
            result = result.replace(&format!("{{{}}}", a), &replacement);
        }
    }
    Ok(result)
}

fn row_to_pack(row: tokio_postgres::Row) -> EmojiPack {
    EmojiPack {
        id: row.get(0),
        owner_user_id: row.get(1),
        name: row.get(2),
        alias: row.get(3),
        is_default: row.get(4),
        item_count: row.get(5),
    }
}

fn row_to_item(row: tokio_postgres::Row) -> EmojiItem {
    EmojiItem {
        id: row.get(0),
        pack_id: row.get(1),
        custom_emoji_id: row.get(2),
        fallback: row.get(3),
        smart_name: row.get(4),
        alias: row.get(5),
        position: row.get(6),
    }
}
