use std::collections::HashMap;
use std::sync::{Arc, RwLock, atomic::AtomicBool};

use crate::stt::types::SttConfig;

#[derive(Debug, Clone)]
pub struct PendingEmoji {
    pub custom_emoji_id: String,
    pub fallback: String,
}

#[derive(Debug, Clone, Default)]
pub enum FlowState {
    #[default]
    Idle,
    AwaitingEmojis {
        collected: Vec<PendingEmoji>,
    },
    AwaitingPackChoice {
        collected: Vec<PendingEmoji>,
    },
    AwaitingPackAlias {
        pack_id: i32,
    },
    AwaitingTestText,
    AwaitingImportFile,
    AwaitingImportMode {
        sql: String,
    },
    AwaitingSttConfig {
        config: SttConfig,
    },
    AwaitingSttAudio {
        config: SttConfig,
    },
    AwaitingDenoiseAudio,
    AwaitingUpscaleImage {
        scale_factor: u32,
        model_name: String,
        anime_expanded: bool,
    },
    AwaitingSeparation,
    AwaitingSeparationMode {
        file_id: String,
        filename: String,
        #[allow(dead_code)]
        prompt_msg_id: Option<i32>,
        is_video: bool,
    },
    AwaitingSeparationQueued {
        cancel: Arc<AtomicBool>,
    },
    AwaitingGeminiWmImage,
    AwaitingPdfCompressFile,
    AwaitingPdfCompressLevel {
        file_id: String,
        filename: String,
    },
    AwaitingIpLookupInput,
    AwaitingSurgeUrlInput,
    AwaitingSurgeConfirm {
        url: String,
        filename: String,
    },
    AwaitingSurgeRenameInput {
        url: String,
        original_filename: String,
        prompt_message_id: i32,
    },
    /// ادمین دکمه‌ی ساخت کد را زده و منتظر آرگومان‌ها (مثل `30d es 1u`) هستیم
    #[allow(dead_code)]
    AwaitingRedeemGenArgs,
    /// ادمین دکمه‌ی «افزودن قفل جدید» را زده و منتظر لینک است
    AwaitingForceJoinLink,
    /// لینک خصوصی ثبت شده؛ منتظر یوزرنیم/فوروارد/آیدی عددی چت هستیم
    AwaitingForceJoinPrivateInfo {
        link: String,
    },
    /// ویزارد ویرایش یک فیلد قفل (نام نمایشی/حد زمان/حد عضو/لینک رزرو).
    /// `field` یکی از: `name` | `time` | `member` | `reserve`. نتیجه به‌صورت پیام جدید نشون داده می‌شه.
    AwaitingForceJoinField {
        lock_id: i64,
        field: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct FlowManager {
    states: Arc<RwLock<HashMap<i64, FlowState>>>,
}

use crate::sync_util::{read_or_recover, write_or_recover};

impl FlowManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, user_id: i64) -> FlowState {
        read_or_recover(&self.states)
            .get(&user_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set(&self, user_id: i64, state: FlowState) {
        let mut map = write_or_recover(&self.states);
        if matches!(state, FlowState::Idle) {
            map.remove(&user_id);
        } else {
            map.insert(user_id, state);
        }
    }

    pub fn clear(&self, user_id: i64) {
        write_or_recover(&self.states).remove(&user_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flow_manager_set_get() {
        let fm = FlowManager::new();
        assert!(matches!(fm.get(123), FlowState::Idle));
        fm.set(123, FlowState::AwaitingDenoiseAudio);
        assert!(matches!(fm.get(123), FlowState::AwaitingDenoiseAudio));
    }

    #[test]
    fn test_flow_manager_clear() {
        let fm = FlowManager::new();
        fm.set(123, FlowState::AwaitingSeparation);
        fm.clear(123);
        assert!(matches!(fm.get(123), FlowState::Idle));
    }

    #[test]
    fn test_flow_manager_idle_removal() {
        let fm = FlowManager::new();
        fm.set(123, FlowState::AwaitingDenoiseAudio);
        fm.set(123, FlowState::Idle);
        assert!(matches!(fm.get(123), FlowState::Idle));
    }
}
