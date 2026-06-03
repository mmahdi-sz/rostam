use super::types::{ParsedItem, ParsedPack, ParsedSql};

pub fn parse_sql(sql: &str) -> ParsedSql {
    let mut result = ParsedSql::default();
    for line in sql.lines() {
        let line = line.trim();
        if line.starts_with("INSERT INTO emoji_packs ") {
            if let Some(pack) = parse_pack_insert(line) { result.packs.push(pack); }
        } else if line.starts_with("INSERT INTO emoji_items ") {
            if let Some(item) = parse_item_insert(line) { result.items.push(item); }
        }
    }
    result
}

fn parse_pack_insert(line: &str) -> Option<ParsedPack> {
    let values_str = extract_values_str(line)?;
    let tokens = parse_values(&values_str);
    if tokens.len() < 5 { return None; }
    Some(ParsedPack {
        old_id: tokens[0].as_deref()?.parse().ok()?,
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
        old_pack_id: tokens[1].as_deref()?.parse().ok()?,
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
        while pos < chars.len() && (chars[pos] == ' ' || chars[pos] == ',') { pos += 1; }
        if pos >= chars.len() { break; }
        if chars[pos] == '\'' {
            pos += 1;
            let mut token = String::new();
            while pos < chars.len() {
                if chars[pos] == '\'' {
                    if pos + 1 < chars.len() && chars[pos + 1] == '\'' { token.push('\''); pos += 2; }
                    else { pos += 1; break; }
                } else { token.push(chars[pos]); pos += 1; }
            }
            result.push(Some(token));
        } else {
            let mut token = String::new();
            while pos < chars.len() && chars[pos] != ',' { token.push(chars[pos]); pos += 1; }
            let token = token.trim().to_string();
            if token.to_uppercase() == "NULL" { result.push(None); } else { result.push(Some(token)); }
        }
    }
    result
}
