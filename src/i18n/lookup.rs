use std::sync::OnceLock;

static I18N: OnceLock<serde_json::Value> = OnceLock::new();

const I18N_JSON: &str = include_str!("../../i18n.json");

pub(super) fn root() -> &'static serde_json::Value {
    I18N.get_or_init(|| serde_json::from_str(I18N_JSON).expect("i18n.json must be valid JSON"))
}

pub fn t(key: &str) -> String {
    let mut node = root();
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
