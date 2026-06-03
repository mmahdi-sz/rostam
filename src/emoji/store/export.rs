use tokio_postgres::Client;

pub async fn export_user_sql(client: &Client, owner: i64) -> Result<String, tokio_postgres::Error> {
    let mut out = String::new();
    out.push_str("-- emoji export\n\n");
    out.push_str("CREATE TABLE IF NOT EXISTS emoji_packs (\n    id SERIAL PRIMARY KEY,\n    owner_user_id BIGINT NOT NULL,\n    name TEXT NOT NULL,\n    alias TEXT,\n    is_default BOOLEAN NOT NULL DEFAULT FALSE,\n    item_count INT NOT NULL DEFAULT 0\n);\n\n");
    out.push_str("CREATE TABLE IF NOT EXISTS emoji_items (\n    id SERIAL PRIMARY KEY,\n    pack_id INT NOT NULL REFERENCES emoji_packs(id) ON DELETE CASCADE,\n    owner_user_id BIGINT NOT NULL,\n    custom_emoji_id TEXT NOT NULL,\n    fallback TEXT NOT NULL,\n    smart_name TEXT NOT NULL,\n    alias TEXT,\n    position INT NOT NULL DEFAULT 0\n);\n\n");

    let packs = client.query(
        "SELECT id, owner_user_id, name, alias, is_default FROM emoji_packs WHERE owner_user_id = $1 ORDER BY id",
        &[&owner],
    ).await?;

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
    if !packs.is_empty() { out.push('\n'); }

    let items = client.query(
        "SELECT id, pack_id, owner_user_id, custom_emoji_id, fallback, smart_name, alias, position FROM emoji_items WHERE owner_user_id = $1 ORDER BY id",
        &[&owner],
    ).await?;

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

pub async fn render_template(client: &Client, owner: i64, template: &str) -> Result<String, tokio_postgres::Error> {
    let rows = client.query(
        "SELECT custom_emoji_id, fallback, smart_name, alias FROM emoji_items WHERE owner_user_id = $1",
        &[&owner],
    ).await?;

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
