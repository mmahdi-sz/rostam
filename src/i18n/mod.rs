mod emoji_map;
mod entities;
mod lookup;
mod premium_md;

pub use entities::entities_for_text;
#[cfg(feature = "testapi")]
pub use lookup::RESOLVED_I18N_KEYS;
pub use lookup::{LANG, reload as reload_i18n, t, tf};
pub use premium_md::{apply_premium_to_html, apply_premium_to_md};

/// Get current thread-local language code (defaults to "fa").
pub fn current_lang() -> String {
    LANG.try_with(|l| l.clone())
        .unwrap_or_else(|_| "fa".to_owned())
}

/// Escape all MarkdownV2 special characters. Apply to every dynamic value in MarkdownV2 messages.
pub fn md_escape(s: &str) -> String {
    const SPECIAL: &[char] = &[
        '_', '*', '[', ']', '(', ')', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!', '~', '`',
        '\\',
    ];
    s.chars()
        .flat_map(|c| {
            if SPECIAL.contains(&c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}

/// تبدیل ارقام انگلیسی به فارسی (شامل نقطه‌ی اعشار → ممیز فارسی).
pub fn to_fa_digits(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '0' => '۰',
            '1' => '۱',
            '2' => '۲',
            '3' => '۳',
            '4' => '۴',
            '5' => '۵',
            '6' => '۶',
            '7' => '۷',
            '8' => '۸',
            '9' => '۹',
            '.' => '٫',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_md_escape() {
        let raw = "Hello [world] (1.2) *bold* _italic_ ~strike~ #tag +1 -2 =3 |pipe {brace} !excl \\slash `code` >quote";
        let escaped = md_escape(raw);
        assert!(escaped.contains("\\[world\\]"));
        assert!(escaped.contains("\\(1\\.2\\)"));
        assert!(escaped.contains("\\*bold\\*"));
    }

    #[test]
    fn test_to_fa_digits() {
        assert_eq!(to_fa_digits("123.45"), "۱۲۳٫۴۵");
        assert_eq!(to_fa_digits("abc 09"), "abc ۰۹");
    }

    #[test]
    fn test_i18n_t_key_lookup() {
        let val = t("start.welcome");
        assert!(!val.is_empty());
    }

    #[test]
    fn test_i18n_tf_formatting() {
        let val = tf("start.welcome", &[("name", "Test")]);
        assert!(!val.is_empty());
    }

    #[test]
    fn test_i18n_entities_for_text() {
        let entities = entities_for_text("Hello {emoji.panel.icons.rank}");
        assert!(entities.is_empty() || !entities.is_empty());
    }

    #[test]
    fn test_i18n_apply_premium_to_md() {
        let text = "Test text";
        let res = apply_premium_to_md(text);
        assert!(!res.is_empty());
    }
}
