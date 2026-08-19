use frankenstein::types::ChatId;

use crate::force_join::jalali::now_epoch;

// Admin panel — "Force join lock" submenu buttons
pub const CB_FJ_TOGGLE: &str = "fj:toggle";
pub const CB_FJ_VIEW: &str = "fj:view";
pub const CB_FJ_NOOP: &str = "fj:nop";
pub const CB_FJ_ADD_NEW: &str = "fj:add";
pub const CB_FJ_ADD_CANCEL: &str = "fj:add_cancel";
pub const CB_FJ_CHECK: &str = "fj:check";
pub const FJ_MANAGE_PREFIX: &str = "fj:manage:";
pub const FJ_MODE_PREFIX: &str = "fj:mode:";
pub const FJ_NAME_PREFIX: &str = "fj:name:";
pub const FJ_TIME_PREFIX: &str = "fj:time:";
pub const FJ_MEMBER_PREFIX: &str = "fj:member:";
pub const FJ_RESERVE_PREFIX: &str = "fj:reserve:";
pub const FJ_DELETE_PREFIX: &str = "fj:delete:";
pub const FJ_DELETE_YES_PREFIX: &str = "fj:del_yes:";

pub struct Lock {
    pub id: i64,
    pub link: String,
    pub identifier: String,
    pub title: String,
    pub display_override: String,
    pub mode: String,
    pub created_at: i64,
    pub expires_at: i64, // 0 = no time limit
    pub member_cap: i64, // 0 = no member limit
    pub reserve_link: String,
}

impl Lock {
    /// Display name: Admin manual override, else chat title (getChat), else identifier, else link.
    pub(crate) fn display_name(&self) -> String {
        if !self.display_override.is_empty() {
            self.display_override.clone()
        } else if !self.title.is_empty() {
            self.title.clone()
        } else if !self.identifier.is_empty() {
            self.identifier.clone()
        } else {
            self.link.clone()
        }
    }

    pub(crate) fn is_mandatory(&self) -> bool {
        self.mode == "mandatory"
    }

    pub(crate) fn chat_id(&self) -> Option<ChatId> {
        chat_id_for(&self.identifier)
    }

    pub(crate) fn is_expired(&self) -> bool {
        self.expires_at != 0 && now_epoch() >= self.expires_at
    }
}

pub(crate) fn chat_id_for(identifier: &str) -> Option<ChatId> {
    if identifier.is_empty() {
        None
    } else if let Ok(n) = identifier.parse::<i64>() {
        Some(ChatId::Integer(n))
    } else if identifier.starts_with('@') {
        Some(ChatId::String(identifier.to_string()))
    } else {
        None
    }
}

pub enum ToggleModeResult {
    Ok,
    /// Lock lacks identifiable chat ID (e.g. Instagram link) — cannot be mandatory.
    NoChatId,
    /// Bot is not admin in the channel/group.
    BotNotAdmin,
    NotFound,
}
