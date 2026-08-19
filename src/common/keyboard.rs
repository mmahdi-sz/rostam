//! Shared inline keyboard generators.
//!
//! Enforces Telegram UI rules (custom emoji handling, danger buttons).

use frankenstein::types::InlineKeyboardMarkup;

use crate::emoji::panel::btn_icon_danger;

/// Generates a standard single-button danger keyboard for job cancellation.
///
/// Adheres to project UI rules: uses `btn_icon_danger` with `icon_custom_emoji_id`.
///
/// # Arguments
/// * `label` - Localized button label (e.g. `&t("denoise.cancel_button")`)
/// * `callback_data` - Strict `{domain}:jobcancel` or `{domain}:cancel` format
/// * `icon_key` - Emoji palette icon key (e.g. `"cancel"`)
pub fn job_cancel_keyboard(
    label: &str,
    callback_data: &str,
    icon_key: &str,
) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(label, callback_data, icon_key)]])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_cancel_keyboard_structure() {
        let kb = job_cancel_keyboard("Cancel Job", "denoise:jobcancel", "cancel");
        assert_eq!(kb.inline_keyboard.len(), 1);
        assert_eq!(kb.inline_keyboard[0].len(), 1);
        let btn = &kb.inline_keyboard[0][0];
        assert_eq!(btn.text, "Cancel Job");
        assert_eq!(btn.callback_data.as_deref(), Some("denoise:jobcancel"));
    }
}
