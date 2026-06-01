pub fn base_smart_name(fallback: &str) -> String {
    let primary = fallback.chars().next().unwrap_or('?');
    let raw = unicode_names2::name(primary)
        .map(|n| n.to_string())
        .unwrap_or_else(|| format!("u{:04X}", primary as u32));

    let mut out = String::with_capacity(raw.len());
    let mut prev_underscore = true;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("emoji");
    }
    out
}
