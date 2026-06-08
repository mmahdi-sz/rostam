use std::sync::{Arc, RwLock, OnceLock};

static I18N: OnceLock<Arc<RwLock<serde_json::Value>>> = OnceLock::new();

fn load_from_file() -> serde_json::Value {
    let json = std::fs::read_to_string("i18n.json")
        .expect("i18n.json not found in working directory");
    serde_json::from_str(&json).expect("i18n.json must be valid JSON")
}

fn try_load_from_file() -> Result<serde_json::Value, String> {
    let json = std::fs::read_to_string("i18n.json")
        .map_err(|e| format!("read i18n.json: {e}"))?;
    serde_json::from_str(&json)
        .map_err(|e| format!("parse i18n.json: {e}"))
}

fn cache() -> &'static Arc<RwLock<serde_json::Value>> {
    I18N.get_or_init(|| Arc::new(RwLock::new(load_from_file())))
}

/// Reload i18n.json from disk without restarting.
/// On parse/IO error keeps existing values and logs — does NOT panic.
pub fn reload() {
    match try_load_from_file() {
        Ok(fresh) => {
            *cache().write().unwrap() = fresh;
            eprintln!("[i18n] reloaded i18n.json from disk");
        }
        Err(e) => eprintln!("[i18n] reload failed, keeping previous values: {e}"),
    }
}

pub fn t(key: &str) -> String {
    let guard = cache().read().unwrap();
    let mut node = &*guard;
    for segment in key.split('.') {
        match node.get(segment) {
            Some(next) => node = next,
            None => return format!("!{key}!"),
        }
    }
    node.as_str().map(|s| s.to_owned()).unwrap_or_else(|| format!("!{key}!"))
}

pub fn tf(key: &str, vars: &[(&str, &str)]) -> String {
    let mut template = t(key);
    for (name, value) in vars {
        let placeholder = format!("{{{name}}}");
        template = template.replace(&placeholder, value);
    }
    template
}
