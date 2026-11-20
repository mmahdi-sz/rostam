use std::collections::HashMap;
use std::sync::{Arc, RwLock, atomic::AtomicBool};

use crate::stt::types::SttConfig;

#[derive(Debug, Clone)]
pub struct PendingEmoji {
    pub custom_emoji_id: String,
    pub fallback: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastMode {
    Copy,
    Forward,
}

#[derive(Debug, Clone, Default)]
pub enum FlowState {
    #[default]
    Idle,
    AwaitingBroadcastBanner {
        mode: BroadcastMode,
        pin: bool,
    },
    AwaitingBroadcastTarget {
        mode: BroadcastMode,
        pin: bool,
        banner_chat_id: i64,
        banner_message_id: i32,
        #[allow(dead_code)]
        total_users: i64,
        #[allow(dead_code)]
        active_users: i64,
    },
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
    AwaitingDeoldifyImage,
    AwaitingNobgImage,
    AwaitingTtsText,
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
    /// Admin clicked generate code button and awaiting arguments (e.g. `30d es 1u`)
    #[allow(dead_code)]
    AwaitingRedeemGenArgs,
    /// Admin clicked "Add New Lock" button and awaiting link
    AwaitingForceJoinLink,
    /// Private link registered; awaiting username/forward/numeric chat ID
    AwaitingForceJoinPrivateInfo {
        link: String,
    },
    /// Wizard editing a lock field (name/time/member/reserve). `field` is one of: `name` | `time` | `member` | `reserve`.
    AwaitingForceJoinField {
        lock_id: i64,
        field: String,
    },
    /// User selecting compression format
    #[allow(dead_code)]
    AwaitingCompressFormatSelect,
    /// Format selected; user configuring options
    AwaitingCompressOptions {
        config: crate::filecompress::CompressConfig,
    },
    /// Awaiting file password input
    AwaitingCompressPassword {
        config: crate::filecompress::CompressConfig,
    },
    /// Awaiting compression input files
    AwaitingCompressFiles {
        config: Box<crate::filecompress::CompressConfig>,
        files: Vec<CompressFileEntry>,
        prompt_msg_id: i32,
    },
    AwaitingStudioTrimVideo,
    AwaitingStudioTrimRanges {
        file_id: String,
        filename: String,
        duration_secs: u64,
    },
    AwaitingStudioCompressVideo,
    AwaitingStudioExtractVideo,
    AwaitingStudioBurnInput {
        session: Arc<std::sync::Mutex<crate::studio::burn::BurnSession>>,
    },
    AwaitingPkgFile,
    AwaitingPkgConvertChoice {
        file_id: String,
        filename: String,
        file_size: u64,
        src_fmt: crate::pkgconvert::detect::PkgFormat,
    },
}

impl FlowState {
    pub fn expects_link(&self) -> bool {
        match self {
            Self::AwaitingSurgeUrlInput | Self::AwaitingForceJoinLink => true,
            Self::AwaitingForceJoinField { field, .. } => field == "reserve",
            Self::Idle
            | Self::AwaitingBroadcastBanner { .. }
            | Self::AwaitingBroadcastTarget { .. }
            | Self::AwaitingEmojis { .. }
            | Self::AwaitingPackChoice { .. }
            | Self::AwaitingPackAlias { .. }
            | Self::AwaitingTestText
            | Self::AwaitingImportFile
            | Self::AwaitingImportMode { .. }
            | Self::AwaitingSttConfig { .. }
            | Self::AwaitingSttAudio { .. }
            | Self::AwaitingDenoiseAudio
            | Self::AwaitingUpscaleImage { .. }
            | Self::AwaitingSeparation
            | Self::AwaitingSeparationMode { .. }
            | Self::AwaitingSeparationQueued { .. }
            | Self::AwaitingGeminiWmImage
            | Self::AwaitingDeoldifyImage
            | Self::AwaitingNobgImage
            | Self::AwaitingTtsText
            | Self::AwaitingPdfCompressFile
            | Self::AwaitingPdfCompressLevel { .. }
            | Self::AwaitingIpLookupInput
            | Self::AwaitingSurgeConfirm { .. }
            | Self::AwaitingSurgeRenameInput { .. }
            | Self::AwaitingRedeemGenArgs
            | Self::AwaitingForceJoinPrivateInfo { .. }
            | Self::AwaitingCompressFormatSelect
            | Self::AwaitingCompressOptions { .. }
            | Self::AwaitingCompressPassword { .. }
            | Self::AwaitingCompressFiles { .. }
            | Self::AwaitingStudioTrimVideo
            | Self::AwaitingStudioTrimRanges { .. }
            | Self::AwaitingStudioCompressVideo
            | Self::AwaitingStudioExtractVideo
            | Self::AwaitingStudioBurnInput { .. }
            | Self::AwaitingPkgFile
            | Self::AwaitingPkgConvertChoice { .. } => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompressFileEntry {
    pub file_id: String,
    pub filename: String,
    #[allow(dead_code)]
    pub size: u64,
}

#[derive(Debug, Clone, Default)]
pub struct FlowManager {
    store: Arc<RwLock<FlowStore>>,
    owner: Option<(i64, u64)>,
}

#[derive(Debug, Default)]
struct FlowStore {
    states: HashMap<i64, FlowState>,
    revisions: HashMap<i64, u64>,
}

impl FlowStore {
    fn revision(&self, user_id: i64) -> u64 {
        self.revisions.get(&user_id).copied().unwrap_or_default()
    }
}

use crate::sync_util::{read_or_recover, write_or_recover};

impl FlowManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn for_user(&self, user_id: i64) -> Self {
        Self {
            store: self.store.clone(),
            owner: Some((user_id, read_or_recover(&self.store).revision(user_id))),
        }
    }

    fn owns(&self, store: &FlowStore, user_id: i64) -> bool {
        self.owner
            .is_none_or(|(owner, revision)| owner == user_id && revision == store.revision(user_id))
    }

    pub(crate) fn is_current(&self, user_id: i64) -> bool {
        self.owns(&read_or_recover(&self.store), user_id)
    }

    pub(crate) fn exit(&mut self, user_id: i64) -> FlowState {
        let mut store = write_or_recover(&self.store);
        if !self.owns(&store, user_id) {
            return FlowState::Idle;
        }
        let revision = store.revision(user_id) + 1;
        store.revisions.insert(user_id, revision);
        self.owner = Some((user_id, revision));
        store.states.remove(&user_id).unwrap_or_default()
    }

    pub fn get(&self, user_id: i64) -> FlowState {
        let store = read_or_recover(&self.store);
        if !self.owns(&store, user_id) {
            return FlowState::Idle;
        }
        store.states.get(&user_id).cloned().unwrap_or_default()
    }

    pub fn set(&self, user_id: i64, state: FlowState) {
        let mut store = write_or_recover(&self.store);
        if !self.owns(&store, user_id) {
            return;
        }
        if matches!(state, FlowState::Idle) {
            store.states.remove(&user_id);
        } else {
            store.states.insert(user_id, state);
        }
    }

    pub fn clear(&self, user_id: i64) {
        self.take(user_id);
    }

    pub(crate) fn clear_if(&self, user_id: i64, matches: impl FnOnce(&FlowState) -> bool) -> bool {
        let mut store = write_or_recover(&self.store);
        if self.owns(&store, user_id) && store.states.get(&user_id).is_some_and(matches) {
            store.states.remove(&user_id);
            return true;
        }
        false
    }

    /// Atomically retrieves and removes the flow state for this user.
    pub fn take(&self, user_id: i64) -> FlowState {
        let mut store = write_or_recover(&self.store);
        if !self.owns(&store, user_id) {
            return FlowState::Idle;
        }
        store.states.remove(&user_id).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exiting_revokes_all_old_handles_for_the_user() {
        let root = FlowManager::new();
        let mut update = root.for_user(42);
        let worker = update.clone();
        update.set(42, FlowState::AwaitingDenoiseAudio);
        assert!(matches!(update.exit(42), FlowState::AwaitingDenoiseAudio));

        worker.set(42, FlowState::AwaitingDenoiseAudio);
        assert!(matches!(root.get(42), FlowState::Idle));
        update.set(42, FlowState::AwaitingTtsText);

        worker.clear(42);
        worker.clear_if(42, |_| true);
        assert!(matches!(worker.take(42), FlowState::Idle));
        assert!(matches!(root.get(42), FlowState::AwaitingTtsText));
        assert!(root.for_user(42).is_current(42));
        assert!(root.for_user(43).is_current(43));
    }

    #[test]
    fn test_flow_manager_take() {
        let fm = FlowManager::new();
        fm.set(42, FlowState::AwaitingDenoiseAudio);
        let state1 = fm.take(42);
        assert!(matches!(state1, FlowState::AwaitingDenoiseAudio));
        let state2 = fm.take(42);
        assert!(matches!(state2, FlowState::Idle));
    }

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
