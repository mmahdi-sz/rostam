use std::{env, fs};

pub fn bot_token() -> Result<String, Box<dyn std::error::Error>> {
    config_value("BOT_TOKEN")
        .ok_or_else(|| "BOT_TOKEN is not set in .env, /etc/default/abc, or process env".into())
}

pub fn admin_user_id() -> Option<i64> {
    config_value("ADMIN_USER_ID")?.parse().ok()
}

pub fn bot_api_base_url() -> Option<String> {
    config_value("BOT_API_BASE_URL")
}

pub fn dev_mode() -> bool {
    config_value("DEV_MODE")
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

pub fn config_value(key: &str) -> Option<String> {
    value_from_env_file(".env", key)
        .or_else(|| value_from_env_file("/etc/default/abc", key))
        .or_else(|| env::var(key).ok())
}

fn value_from_env_file(path: &str, target_key: &str) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    contents.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        if key.trim() != target_key {
            return None;
        }
        let token = unquote_env_value(value.trim());
        if token.is_empty() { None } else { Some(token.to_owned()) }
    })
}

fn unquote_env_value(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}
