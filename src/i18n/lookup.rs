use std::sync::{Arc, RwLock, OnceLock};

static I18N: OnceLock<Arc<RwLock<serde_json::Value>>> = OnceLock::new();

fn load_from_file() -> serde_json::Value {
    let json = std::fs::read_to_string("i18n.json")
        .expect("i18n.json not found in working directory");
    serde_json::from_str(&json).expect("i18n.json must be valid JSON")
}

fn cache() -> &'static Arc<RwLock<serde_json::Value>> {
    I18N.get_or_init(|| Arc::new(RwLock::new(load_from_file())))
}

/// Reload i18n.json from disk without restarting.
pub fn reload() {
    let fresh = load_from_file();
    *cache().write().unwrap() = fresh;
    eprintln!("[i18n] reloaded i18n.json from disk");
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
