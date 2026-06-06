use crate::cookie_pool::{CookiePool, CookieSource};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::FlowManager;
use frankenstein::client_reqwest::Bot;
use tokio::sync::mpsc::UnboundedSender;

pub struct AppState {
    pub api: Bot,
    pub cookie_pool: CookiePool,
    pub database: Option<PostgresDatabase>,
    pub flow_manager: FlowManager,
    pub rate_limit_tx: UnboundedSender<CookieSource>,
}
