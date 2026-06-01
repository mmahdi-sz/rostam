use tokio_postgres::Client;

#[derive(Debug, Clone)]
pub struct ParsedPack {
    pub old_id: i32,
    pub name: String,
    pub alias: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedItem {
    pub old_pack_id: i32,
    pub custom_emoji_id: String,
    pub fallback: String,
    pub smart_name: String,
    pub alias: Option<String>,
    pub position: i32,
}

#[derive(Debug, Default, Clone)]
pub struct ParsedSql {
    pub packs: Vec<ParsedPack>,
    pub items: Vec<ParsedItem>,
}

#[derive(Debug)]
pub struct ImportAnalysis {
    pub file_packs: usize,
    pub file_items: usize,
    pub db_packs: usize,
    pub db_items: usize,
    pub duplicate_items: usize,
    pub db_empty: bool,
}

#[derive(Debug)]
pub struct ImportResult {
    pub packs_added: usize,
    pub items_added: usize,
    pub items_skipped: usize,
}

pub fn parse_sql(sql: &str) -> ParsedSql {
    let mut result = ParsedSql::default();
    for line in sql.lines() {
        let line = line.trim();
        if line.starts_with("INSERT INTO emoji_packs ") {
            if let Some(pack) = parse_pack_insert(line) {
                result.packs.push(pack);
            }
        } else if line.starts_with("INSERT INTO emoji_items ") {
            if let Some(item) = parse_item_insert(line) {
                result.items.push(item);
            }
        }
    }
    result
}

pub async fn analyze(
    parsed: &ParsedSql,
    client: &Client,
    owner: i64,
) -> ImportAnalysis {
    let db_packs = client
        .query_one("SELECT COUNT(*) FROM emoji_packs WHERE owner_user_id = $1", &[&owner])
        .await
        .map(|r| r.get::<_, i64>(0) as usize)
        .unwrap_or(0);

    let db_items = client
        .query_one("SELECT COUNT(*) FROM emoji_items WHERE owner_user_id = $1", &[&owner])
        .await
        .map(|r| r.get::<_, i64>(0) as usize)
        .unwrap_or(0);

    let file_ids: Vec<String> = parsed.items.iter().map(|i| i.custom_emoji_id.clone()).collect();
    let duplicate_items = if file_ids.is_empty() || db_items == 0 {
        0
    } else {
        client
            .query_one(
                "SELECT COUNT(*) FROM emoji_items WHERE owner_user_id = $1 AND custom_emoji_id = ANY($2)",
                &[&owner, &file_ids],
            )
            .await
            .map(|r| r.get::<_, i64>(0) as usize)
            .unwrap_or(0)
    };

    let db_empty = db_packs == 0;
    ImportAnalysis {
        file_packs: parsed.packs.len(),
        file_items: parsed.items.len(),
        db_packs,
        db_items,
        duplicate_items,
        db_empty,
    }
}

pub async fn execute_replace(
    parsed: &ParsedSql,
    client: &Client,
    owner: i64,
) -> Result<ImportResult, tokio_postgres::Error> {
    client.execute("DELETE FROM emoji_packs WHERE owner_user_id = $1", &[&owner]).await?;

    insert_parsed(parsed, client, owner, false).await
}

pub async fn execute_merge(
    parsed: &ParsedSql,
    client: &Client,
    owner: i64,
    skip_duplicates: bool,
) -> Result<ImportResult, tokio_postgres::Error> {
    insert_parsed(parsed, client, owner, skip_duplicates).await
}

async fn insert_parsed(
    parsed: &ParsedSql,
    client: &Client,
    owner: i64,
    skip_duplicates: bool,
) -> Result<ImportResult, tokio_postgres::Error> {
    let mut pack_id_map: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
    let mut packs_added = 0usize;

    for pack in &parsed.packs {
        let existing = client
            .query_opt(
                "SELECT id FROM emoji_packs WHERE owner_user_id = $1 AND name = $2",
                &[&owner, &pack.name],
            )
            .await?;

        let new_id = if let Some(row) = existing {
            row.get::<_, i32>(0)
        } else {
            let row = client
                .query_one(
                    "INSERT INTO emoji_packs (owner_user_id, name, alias, is_default)
                     VALUES ($1, $2, $3, $4)
                     RETURNING id",
                    &[&owner, &pack.name, &pack.alias, &pack.is_default],
                )
                .await?;
            packs_added += 1;
            row.get::<_, i32>(0)
        };
        pack_id_map.insert(pack.old_id, new_id);
    }

    let existing_ids: std::collections::HashSet<String> = if skip_duplicates {
        client
            .query(
                "SELECT custom_emoji_id FROM emoji_items WHERE owner_user_id = $1",
                &[&owner],
            )
            .await?
            .into_iter()
            .map(|r| r.get::<_, String>(0))
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    let mut items_added = 0usize;
    let mut items_skipped = 0usize;

    for item in &parsed.items {
        let Some(&new_pack_id) = pack_id_map.get(&item.old_pack_id) else {
            items_skipped += 1;
            continue;
        };

        if skip_duplicates && existing_ids.contains(&item.custom_emoji_id) {
            items_skipped += 1;
            continue;
        }

        let smart = allocate_smart_name(client, owner, &item.fallback).await?;

        if let Err(e) = client
            .execute(
                "INSERT INTO emoji_items (pack_id, owner_user_id, custom_emoji_id, fallback, smart_name, alias, position)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[&new_pack_id, &owner, &item.custom_emoji_id, &item.fallback, &smart, &item.alias, &item.position],
            )
            .await
        {
            eprintln!("insert item failed: {e}");
            items_skipped += 1;
            continue;
        }
        items_added += 1;
    }

    Ok(ImportResult { packs_added, items_added, items_skipped })
}

async fn allocate_smart_name(
    client: &Client,
    owner: i64,
    fallback: &str,
) -> Result<String, tokio_postgres::Error> {
    let base = super::smart_name::base_smart_name(fallback);
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
    let max_n: i32 = row.get(0);
    Ok(format!("{}{}", base, max_n + 1))
}

// --- SQL parser ---

fn parse_pack_insert(line: &str) -> Option<ParsedPack> {
    let values_str = extract_values_str(line)?;
    let tokens = parse_values(&values_str);
    if tokens.len() < 5 { return None; }
    Some(ParsedPack {
        old_id: tokens[0].as_deref()?.parse().ok()?,
        // tokens[1] is owner_user_id — skip
        name: tokens[2].clone()?,
        alias: tokens[3].clone(),
        is_default: tokens[4].as_deref()? == "true",
    })
}

fn parse_item_insert(line: &str) -> Option<ParsedItem> {
    let values_str = extract_values_str(line)?;
    let tokens = parse_values(&values_str);
    if tokens.len() < 8 { return None; }
    Some(ParsedItem {
        // tokens[0] is id — skip
        old_pack_id: tokens[1].as_deref()?.parse().ok()?,
        // tokens[2] is owner_user_id — skip
        custom_emoji_id: tokens[3].clone()?,
        fallback: tokens[4].clone()?,
        smart_name: tokens[5].clone()?,
        alias: tokens[6].clone(),
        position: tokens[7].as_deref()?.parse().unwrap_or(0),
    })
}

fn extract_values_str(line: &str) -> Option<&str> {
    let start = line.find("VALUES (")? + "VALUES (".len();
    let end = line.rfind(')')?;
    if end <= start { return None; }
    Some(&line[start..end])
}

fn parse_values(s: &str) -> Vec<Option<String>> {
    let mut result = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut pos = 0;

    while pos < chars.len() {
        while pos < chars.len() && (chars[pos] == ' ' || chars[pos] == ',') {
            pos += 1;
        }
        if pos >= chars.len() { break; }

        if chars[pos] == '\'' {
            pos += 1;
            let mut token = String::new();
            while pos < chars.len() {
                if chars[pos] == '\'' {
                    if pos + 1 < chars.len() && chars[pos + 1] == '\'' {
                        token.push('\'');
                        pos += 2;
                    } else {
                        pos += 1;
                        break;
                    }
                } else {
                    token.push(chars[pos]);
                    pos += 1;
                }
            }
            result.push(Some(token));
        } else {
            let mut token = String::new();
            while pos < chars.len() && chars[pos] != ',' {
                token.push(chars[pos]);
                pos += 1;
            }
            let token = token.trim().to_string();
            if token.to_uppercase() == "NULL" {
                result.push(None);
            } else {
                result.push(Some(token));
            }
        }
    }
    result
}
