//! پنل گرافیکی ساخت کد هدیه (ادمین): انتخاب مقام/مدت/تعداد با دکمه‌ها.

use frankenstein::types::{ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup};

use crate::i18n::to_fa_digits;
use crate::rank::types::Rank;

use super::panel_state::GenSelection;

// ── callback constants ──
pub const CB_GC_PREFIX: &str = "gc:";
pub const CB_GC_RANK: &str = "gc:r:";   // gc:r:{rank_str}
pub const CB_GC_DAYS: &str = "gc:d:";   // gc:d:{days}
pub const CB_GC_USES: &str = "gc:u:";   // gc:u:{uses}
pub const CB_GC_GO: &str = "gc:go";
pub const CB_GC_NOP: &str = "gc:nop";

// چیدمان‌ها
const RANKS: &[Rank] = &[Rank::Sepahbod, Rank::Esfandyar, Rank::Sohrab, Rank::Rostam];
const DAYS: &[i32] = &[15, 31, 45, 60];

fn plain(text: String, cb: &str) -> InlineKeyboardButton {
    btn(text, cb.to_string(), None)
}

fn btn(text: String, cb: String, style: Option<ButtonStyle>) -> InlineKeyboardButton {
    InlineKeyboardButton {
        text,
        icon_custom_emoji_id: None,
        callback_data: Some(cb),
        style,
        url: None, login_url: None, web_app: None,
        switch_inline_query: None, switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None, copy_text: None,
        callback_game: None, pay: None,
    }
}

/// دکمه‌ی انتخابی: انتخاب‌شده → سبز (Success)
fn choice(text: String, cb: String, selected: bool) -> InlineKeyboardButton {
    btn(text, cb, if selected { Some(ButtonStyle::Success) } else { None })
}

fn header(text: &str) -> InlineKeyboardButton {
    plain(text.to_string(), CB_GC_NOP)
}

/// متن بالای پنل با خلاصه‌ی انتخاب فعلی
pub fn panel_text(sel: &GenSelection) -> String {
    to_fa_digits(&format!(
        "🎁 ساخت کد هدیه\n\nمقام: {}\nمدت: {} روز\nتعداد مصرف: {} عدد\n\nانتخاب‌ها را بزن و «ساخت کد» را لمس کن.",
        sel.rank.display_name(),
        sel.days,
        sel.uses,
    ))
}

/// کیبورد پنل بر اساس انتخاب فعلی
pub fn build_keyboard(sel: &GenSelection) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // ── مقام (۲×۲) ──
    rows.push(vec![header("— مقام —")]);
    for pair in RANKS.chunks(2) {
        let row: Vec<InlineKeyboardButton> = pair
            .iter()
            .map(|r| {
                choice(
                    r.display_name().to_string(),
                    format!("{CB_GC_RANK}{}", r.as_str()),
                    sel.rank == *r,
                )
            })
            .collect();
        rows.push(row);
    }

    // ── مدت (۲×۲) ──
    rows.push(vec![header("— مدت (روز) —")]);
    for pair in DAYS.chunks(2) {
        let row: Vec<InlineKeyboardButton> = pair
            .iter()
            .map(|d| {
                choice(
                    to_fa_digits(&format!("{d} روز")),
                    format!("{CB_GC_DAYS}{d}"),
                    sel.days == *d,
                )
            })
            .collect();
        rows.push(row);
    }

    // ── تعداد مصرف (۲ ردیف × ۵ ستون: ۱..۱۰) ──
    rows.push(vec![header("— تعداد مصرف —")]);
    for chunk in (1..=10).collect::<Vec<i32>>().chunks(5) {
        let row: Vec<InlineKeyboardButton> = chunk
            .iter()
            .map(|n| {
                choice(
                    to_fa_digits(&n.to_string()),
                    format!("{CB_GC_USES}{n}"),
                    sel.uses == *n,
                )
            })
            .collect();
        rows.push(row);
    }

    // ── تایید + برگشت ──
    rows.push(vec![btn("✅ ساخت کد".to_string(), CB_GC_GO.to_string(), Some(ButtonStyle::Success))]);
    rows.push(vec![plain("🔙 برگشت".to_string(), crate::bot::CB_ADMIN_PANEL)]);

    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}
