use crate::i18n::t;

/// Human-readable language name for a language code, translated into the
/// current user's UI language via i18n.json (youtube.selection.lang_names.*).
/// Falls back to the bare code if no translation exists at all.
pub fn lang_name(code: &str) -> String {
    let lower = code.to_ascii_lowercase();
    let full = t(&format!("youtube.selection.lang_names.{lower}"));
    if !full.starts_with('!') {
        return full;
    }
    if let Some(dash) = lower.find('-') {
        let prefix = &lower[..dash];
        let short = t(&format!("youtube.selection.lang_names.{prefix}"));
        if !short.starts_with('!') {
            return short;
        }
    }
    code.to_string()
}
