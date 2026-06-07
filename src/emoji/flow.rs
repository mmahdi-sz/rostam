use std::collections::HashMap;
use std::sync::{Arc, atomic::AtomicBool};

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
    AwaitingImportMode { sql: String },
    AwaitingSttConfig { config: SttConfig },
    AwaitingSttAudio { config: SttConfig },
    AwaitingDenoiseAudio,
    AwaitingUpscaleImage { scale_factor: u32, model_name: String, anime_expanded: bool },
    AwaitingSeparation,
    AwaitingSeparationMode { file_id: String, filename: String, prompt_msg_id: Option<i32>, is_video: bool },
    AwaitingSeparationQueued { cancel: Arc<AtomicBool> },
    AwaitingGeminiWmImage,
}

#[derive(Debug, Default)]
pub struct FlowManager {
    states: HashMap<i64, FlowState>,
}

impl FlowManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, user_id: i64) -> FlowState {
        self.states.get(&user_id).cloned().unwrap_or_default()
    }

    pub fn set(&mut self, user_id: i64, state: FlowState) {
        if matches!(state, FlowState::Idle) {
            self.states.remove(&user_id);
        } else {
            self.states.insert(user_id, state);
        }
    }

    pub fn clear(&mut self, user_id: i64) {
        self.states.remove(&user_id);
    }
}
