mod emoji_map;
mod lookup;
mod entities;
mod premium_md;

pub use emoji_map::EMOJI_MAP;
pub use lookup::{t, tf, reload as reload_i18n};
pub use entities::entities_for_text;
pub use premium_md::{apply_premium_to_md, apply_premium_to_html};

/// تبدیل ارقام انگلیسی به فارسی (شامل نقطه‌ی اعشار → ممیز فارسی).
pub fn to_fa_digits(s: &str) -> String {
    s.chars().map(|c| match c {
        '0' => '۰', '1' => '۱', '2' => '۲', '3' => '۳', '4' => '۴',
        '5' => '۵', '6' => '۶', '7' => '۷', '8' => '۸', '9' => '۹',
        '.' => '٫',
        other => other,
    }).collect()
}
