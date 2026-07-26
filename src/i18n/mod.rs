mod emoji_map;
mod lookup;
mod entities;
mod premium_md;

#[cfg(feature = "testapi")]
pub use lookup::RESOLVED_I18N_KEYS;
pub use lookup::{t, tf, reload as reload_i18n, LANG};
pub use entities::entities_for_text;
pub use premium_md::{apply_premium_to_md, apply_premium_to_html};

/// Escape all MarkdownV2 special characters. Apply to every dynamic value in MarkdownV2 messages.
pub fn md_escape(s: &str) -> String {
    const SPECIAL: &[char] = &['_','*','[',']','(',')','>','#','+','-','=','|','{','}','.','!','~','`','\\'];
    s.chars().flat_map(|c| if SPECIAL.contains(&c) { vec!['\\', c] } else { vec![c] }).collect()
}

/// تبدیل ارقام انگلیسی به فارسی (شامل نقطه‌ی اعشار → ممیز فارسی).
pub fn to_fa_digits(s: &str) -> String {
    s.chars().map(|c| match c {
        '0' => '۰', '1' => '۱', '2' => '۲', '3' => '۳', '4' => '۴',
        '5' => '۵', '6' => '۶', '7' => '۷', '8' => '۸', '9' => '۹',
        '.' => '٫',
        other => other,
    }).collect()
}
