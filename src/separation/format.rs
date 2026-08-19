use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::DeleteMessageParams,
};

use crate::i18n::tf;

/// Formats seconds into mm:ss or hh:mm:ss.
pub fn format_clock(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Formats duration into Persian localized string.
pub fn format_duration_fa(secs: u64) -> String {
    if secs < 3600 {
        let mins = secs / 60;
        tf("rank.duration_minutes", &[("mins", &mins.to_string())])
    } else {
        let hours = secs / 3600;
        let rem_mins = (secs % 3600) / 60;
        if rem_mins == 0 {
            tf("rank.duration_hours", &[("hours", &hours.to_string())])
        } else {
            tf(
                "rank.duration_hours_minutes",
                &[
                    ("hours", &hours.to_string()),
                    ("mins", &rem_mins.to_string()),
                ],
            )
        }
    }
}

/// Deletes a Telegram message.
pub async fn delete_message(api: &Bot, chat_id: i64, message_id: i32) -> crate::error::Result<()> {
    let params = DeleteMessageParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .build();
    api.delete_message(&params).await?;
    Ok(())
}
