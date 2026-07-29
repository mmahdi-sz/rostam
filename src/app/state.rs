use crate::cookie_pool::{CookiePool, CookieSource};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::FlowManager;
use frankenstein::client_reqwest::Bot;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

pub struct AppState {
    pub api: Bot,
    pub cookie_pool: Arc<Mutex<CookiePool>>,
    pub database: Option<PostgresDatabase>,
    pub flow_manager: FlowManager,
    pub rate_limit_tx: UnboundedSender<CookieSource>,
    /// Spawned separation/STT tasks send the user_id here when finished
    /// so the main loop can clear their FlowState.
    pub flow_clear_tx: UnboundedSender<i64>,
    pub user_last_update: Arc<Mutex<std::collections::HashMap<i64, std::time::Instant>>>,
}
